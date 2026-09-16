//! JNI owns no Java pointers. Opaque monotonically allocated IDs index engines.
//! Registry locking serializes process/destroy; stale IDs fail safely. Each call
//! copies inputs and returns a fresh Java array/string, owned by the JVM.
use crate::{
    engine::{Engine, Packet},
    policy, Result,
};
use jni::{
    objects::{JByteArray, JClass, JString},
    sys::{jbyteArray, jint, jlong, jstring},
    JNIEnv,
};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::atomic::{AtomicI64, Ordering},
    sync::{Mutex, OnceLock},
};
static ENGINES: OnceLock<Mutex<HashMap<i64, Engine>>> = OnceLock::new();
static NEXT: AtomicI64 = AtomicI64::new(1);
fn command(op: &str, input: &str) -> Result<Value> {
    let v: Value = serde_json::from_str(input).map_err(|_| "Invalid JNI JSON")?;
    let engines = ENGINES.get_or_init(|| Mutex::new(HashMap::new()));
    match op {
        "keys" => {
            let (private, public) = crate::engine::keypair();
            Ok(json!({"private": private,"public":public}))
        }
        "plan" => {
            let config =
                serde_json::from_value(v["config"].clone()).map_err(|_| "Invalid server config")?;
            let local: Vec<String> = serde_json::from_value(v["local"].clone())
                .map_err(|_| "Invalid physical addresses")?;
            serde_json::to_value(policy::plan(&config, &local)?).map_err(|_| "Invalid plan".into())
        }
        "create" => {
            let config =
                serde_json::from_value(v["config"].clone()).map_err(|_| "Invalid server config")?;
            let engine = Engine::new(v["private"].as_str().ok_or("Missing private key")?, &config)?;
            let id = NEXT.fetch_add(1, Ordering::Relaxed);
            engines
                .lock()
                .map_err(|_| "Engine lock poisoned")?
                .insert(id, engine);
            Ok(json!({"id":id}))
        }
        "destroy" => {
            engines
                .lock()
                .map_err(|_| "Engine lock poisoned")?
                .remove(&v["id"].as_i64().ok_or("Missing handle")?);
            Ok(json!({}))
        }
        "stats" => {
            let map = engines.lock().map_err(|_| "Engine lock poisoned")?;
            let engine = map
                .get(&v["id"].as_i64().ok_or("Missing handle")?)
                .ok_or("Closed engine")?;
            serde_json::to_value(engine.stats()).map_err(|_| "Invalid stats".into())
        }
        _ => Err("Unknown JNI command".into()),
    }
}
#[no_mangle]
pub extern "system" fn Java_com_biubiu_vpn_Native_call(
    mut env: JNIEnv,
    _: JClass,
    op: JString,
    input: JString,
) -> jstring {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let op: String = env.get_string(&op).map_err(|_| "Invalid operation")?.into();
        let input: String = env.get_string(&input).map_err(|_| "Invalid input")?.into();
        command(&op, &input)
    }))
    .unwrap_or_else(|_| Err("Native engine failed".into()));
    let value = match result {
        Ok(v) => json!({"data":v}),
        Err(e) => json!({"error":e}),
    };
    env.new_string(value.to_string())
        .map(|s| s.into_raw())
        .unwrap_or(std::ptr::null_mut())
}
#[no_mangle]
pub extern "system" fn Java_com_biubiu_vpn_Native_process(
    mut env: JNIEnv,
    _: JClass,
    id: jlong,
    kind: jint,
    input: JByteArray,
) -> jbyteArray {
    let result: Result<Vec<u8>> = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let input = env
            .convert_byte_array(input)
            .map_err(|_| "Invalid packet")?;
        let mut map = ENGINES
            .get_or_init(|| Mutex::new(HashMap::new()))
            .lock()
            .map_err(|_| "Engine lock poisoned")?;
        let engine = map.get_mut(&id).ok_or("Closed engine")?;
        let packets = engine.process(kind as u8, &input)?;
        let mut output = Vec::new();
        for packet in packets {
            let (kind, bytes) = match packet {
                Packet::Network(b) => (0, b),
                Packet::Tunnel(b) => (1, b),
            };
            output.push(kind);
            output.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
            output.extend_from_slice(&bytes);
        }
        Ok(output)
    }))
    .unwrap_or_else(|_| Err("Native engine failed".into()));
    match result {
        Ok(bytes) => env
            .byte_array_from_slice(&bytes)
            .map(|a| a.into_raw())
            .unwrap_or(std::ptr::null_mut()),
        Err(message) => {
            let _ = env.throw_new("java/lang/IllegalStateException", message);
            std::ptr::null_mut()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stale_handle_cannot_refer_to_a_new_engine() {
        let keys = command("keys", "{}").unwrap();
        let config = json!({"vpn_ip":"10.8.0.2","vpn_subnet":"10.8.0.0/24","server_public_key":keys["public"],"server_endpoint":"vpn.example.com:51820"});
        let input = json!({"private":keys["private"],"config":config}).to_string();
        let first = command("create", &input).unwrap();
        command("destroy", &first.to_string()).unwrap();
        let second = command("create", &input).unwrap();
        assert_ne!(first["id"], second["id"]);
        assert!(command("stats", &first.to_string()).is_err());
        assert!(command("stats", &second.to_string()).is_ok());
        command("destroy", &second.to_string()).unwrap();
    }
}
