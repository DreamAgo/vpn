use std::str::FromStr;

use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use uuid::Uuid;
use vpn_server::repositories::{
    ApprovalIdentity, ApprovedGrant, EnqueueResult, SqliteAccessGrantRepository,
    SqliteUserGroupRepository, SqliteUserRepository,
};

async fn pool() -> sqlx::SqlitePool {
    let url = format!(
        "sqlite:file:access_grant_{}?mode=memory&cache=shared",
        Uuid::new_v4()
    );
    let pool = SqlitePoolOptions::new()
        .max_connections(4)
        .connect_with(SqliteConnectOptions::from_str(&url).unwrap())
        .await
        .unwrap();
    sqlx::migrate!("../../migrations").run(&pool).await.unwrap();
    pool
}

fn grant<'a>(
    instance_code: &'a str,
    group_id: &'a str,
    expires_at: i64,
    subject: &'a str,
    email: &'a str,
    user_id: &'a str,
) -> ApprovedGrant<'a> {
    ApprovedGrant {
        max_devices: None,
        instance_code,
        group_ids: vec![group_id.to_string()],
        expires_at,
        reason: "project",
        identity: ApprovalIdentity {
            subject,
            email,
            preferred_username: "alice",
            username_suffix: "deadbeef",
            new_user_id: user_id,
            password_hash: "unknown-password-hash",
        },
    }
}

#[tokio::test]
async fn approved_instance_atomically_creates_restricted_identity_and_never_shortens() {
    let pool = pool().await;
    sqlx::query(
        "INSERT INTO user_groups(id,name,routes,created_at,updated_at) VALUES('g1','prod','192.168.10.0/24',0,0)",
    )
    .execute(&pool)
    .await
    .unwrap();
    let repo = SqliteAccessGrantRepository::new(pool.clone());
    repo.apply_approved(grant(
        "instance-1",
        "g1",
        2_000_000_000_000,
        "union-1",
        "alice@example.com",
        "user-1",
    ))
    .await
    .unwrap();

    let user = SqliteUserRepository::new(pool.clone())
        .find_by_external_identity("feishu", "union-1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(user.id, "user-1");
    assert_eq!(user.access_mode, "approval_required");
    assert!(user.group_ids.is_empty(), "审批不得写入人工组成员表");
    let saved: (i64,) = sqlx::query_as(
        "SELECT expires_at FROM access_grants WHERE approval_instance_code='instance-1'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(saved.0, 2_000_000_000_000);

    repo.apply_approved(grant(
        "instance-1",
        "g1",
        1_900_000_000_000,
        "union-1",
        "alice@example.com",
        "unused-user",
    ))
    .await
    .unwrap();
    let unchanged: (i64,) = sqlx::query_as(
        "SELECT expires_at FROM access_grants WHERE approval_instance_code='instance-1'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(unchanged.0, 2_000_000_000_000);
}

#[tokio::test]
async fn effective_routes_union_manual_and_unexpired_approval_without_legacy_fallback() {
    let pool = pool().await;
    let users = SqliteUserRepository::new(pool.clone());
    users
        .insert("u1", "alice", "alice@example.com", "h", "user", false, 1)
        .await
        .unwrap();
    sqlx::query("UPDATE users SET access_mode='approval_required' WHERE id='u1'")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        r#"INSERT INTO user_groups(id,name,routes,created_at,updated_at) VALUES
           ('manual','manual','10.1.0.0/16',0,0),
           ('approved','approved','10.2.0.0/16',0,0)"#,
    )
    .execute(&pool)
    .await
    .unwrap();
    let groups = SqliteUserGroupRepository::new(pool.clone());
    assert_eq!(groups.routes_for_user("u1").await.unwrap(), Some(vec![]));
    sqlx::query("INSERT INTO user_group_members(user_id,group_id) VALUES('u1','manual')")
        .execute(&pool)
        .await
        .unwrap();
    let now = chrono::Utc::now().timestamp_millis();
    sqlx::query(
        r#"INSERT INTO access_grants(id,approval_instance_code,user_id,group_id,expires_at,reason,created_at,updated_at)
           VALUES('a1','i1','u1','approved',?1,'',0,0),
                 ('a2','i2','u1','approved',?2,'',0,0)"#,
    )
    .bind(now + 60_000)
    .bind(now - 1)
    .execute(&pool)
    .await
    .unwrap();
    let mut routes = groups.routes_for_user("u1").await.unwrap().unwrap();
    routes.sort();
    assert_eq!(routes, vec!["10.1.0.0/16", "10.2.0.0/16"]);
}

#[tokio::test]
async fn durable_inbox_distinguishes_safe_replay_from_changed_payload() {
    let pool = pool().await;
    let repo = SqliteAccessGrantRepository::new(pool);
    assert_eq!(
        repo.enqueue("evt-1", "instance-1", "hash-a", "{}")
            .await
            .unwrap(),
        EnqueueResult::Inserted
    );
    assert_eq!(
        repo.enqueue("evt-1", "instance-1", "hash-a", "{}")
            .await
            .unwrap(),
        EnqueueResult::Duplicate
    );
    assert_eq!(
        repo.enqueue("evt-1", "instance-1", "hash-b", "{}")
            .await
            .unwrap(),
        EnqueueResult::Conflict
    );
    assert_eq!(
        repo.enqueue("evt-2", "instance-1", "hash-a", "{}")
            .await
            .unwrap(),
        EnqueueResult::Duplicate
    );
    assert_eq!(
        repo.enqueue("evt-3", "instance-1", "hash-b", "{}")
            .await
            .unwrap(),
        EnqueueResult::Conflict
    );
}

#[tokio::test]
async fn concurrent_instance_replays_create_only_one_inbox_item() {
    let pool = pool().await;
    let first = SqliteAccessGrantRepository::new(pool.clone());
    let second = SqliteAccessGrantRepository::new(pool.clone());
    let (left, right) = tokio::join!(
        first.enqueue("evt-a", "instance-race", "stable-hash", "{}"),
        second.enqueue("evt-b", "instance-race", "stable-hash", "{}"),
    );
    let results = [left.unwrap(), right.unwrap()];
    assert!(results.contains(&EnqueueResult::Inserted));
    assert!(results.contains(&EnqueueResult::Duplicate));
    let count: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM feishu_approval_inbox WHERE instance_code='instance-race'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count.0, 1);
}
