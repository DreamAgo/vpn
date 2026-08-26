//! 仅监听 VPN 网关地址的内置 DNS 转发器。

use std::{
    collections::{HashMap, VecDeque},
    net::{Ipv4Addr, SocketAddr},
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{anyhow, Context};
use hickory_proto::{
    op::{Message, MessageType, OpCode, ResponseCode},
    rr::{rdata::A, RData, Record, RecordType},
};
use ipnet::Ipv4Net;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream, UdpSocket},
    sync::{Mutex, RwLock, Semaphore},
    time::timeout,
};
use vpn_api_types::system::{normalize_dns_domain, ClientDnsMode, DnsNetworkSettings};

const UPSTREAM_TIMEOUT: Duration = Duration::from_secs(2);
const TCP_CLIENT_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_DNS_PACKET: usize = 65_535;
const CACHE_LIMIT: usize = 2_048;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CacheKey {
    request: Vec<u8>,
    upstreams: String,
}

struct CacheEntry {
    packet: Vec<u8>,
    expires_at: Instant,
}

#[derive(Default)]
struct DnsCache {
    entries: HashMap<CacheKey, CacheEntry>,
    order: VecDeque<CacheKey>,
}

impl DnsCache {
    fn get(&mut self, key: &CacheKey, id: u16) -> Option<Vec<u8>> {
        let now = Instant::now();
        let entry = self.entries.get(key)?;
        if entry.expires_at <= now {
            self.entries.remove(key);
            self.order.retain(|candidate| candidate != key);
            return None;
        }
        let remaining = entry.expires_at.duration_since(now).as_secs().max(1) as u32;
        let mut message = Message::from_vec(&entry.packet).ok()?;
        message.metadata.id = id;
        for record in message
            .answers
            .iter_mut()
            .chain(message.authorities.iter_mut())
            .chain(message.additionals.iter_mut())
        {
            record.ttl = record.ttl.min(remaining);
        }
        message.to_vec().ok()
    }

    fn insert(&mut self, key: CacheKey, mut packet: Vec<u8>, ttl: Duration) {
        if packet.len() < 2 {
            return;
        }
        packet[..2].copy_from_slice(&0_u16.to_be_bytes());
        if !self.entries.contains_key(&key) {
            self.order.push_back(key.clone());
        }
        self.entries.insert(
            key,
            CacheEntry {
                packet,
                expires_at: Instant::now() + ttl,
            },
        );
        while self.entries.len() > CACHE_LIMIT {
            if let Some(oldest) = self.order.pop_front() {
                self.entries.remove(&oldest);
            } else {
                break;
            }
        }
    }
}

#[derive(Clone)]
pub struct DnsServer {
    gateway: Ipv4Addr,
    subnet: Ipv4Net,
    settings: Arc<RwLock<DnsNetworkSettings>>,
    cache: Arc<Mutex<DnsCache>>,
    concurrency: Arc<Semaphore>,
}

impl DnsServer {
    pub fn new(subnet: Ipv4Net, settings: Arc<RwLock<DnsNetworkSettings>>) -> anyhow::Result<Self> {
        let gateway = subnet
            .hosts()
            .next()
            .ok_or_else(|| anyhow!("VPN 子网没有可用的 DNS 网关地址"))?;
        Ok(Self {
            gateway,
            subnet,
            settings,
            cache: Arc::new(Mutex::new(DnsCache::default())),
            concurrency: Arc::new(Semaphore::new(256)),
        })
    }

    pub fn gateway(&self) -> Ipv4Addr {
        self.gateway
    }

    pub async fn run(self) -> anyhow::Result<()> {
        if self.settings.read().await.mode == ClientDnsMode::Disabled {
            tracing::info!(gateway = %self.gateway, "VPN 内置 DNS 当前关闭，等待后台启用");
            loop {
                tokio::time::sleep(Duration::from_millis(500)).await;
                if self.settings.read().await.mode != ClientDnsMode::Disabled {
                    break;
                }
            }
        }
        let address = SocketAddr::from((self.gateway, 53));
        let udp = UdpSocket::bind(address)
            .await
            .with_context(|| format!("绑定 VPN DNS UDP {address} 失败"))?;
        let tcp = TcpListener::bind(address)
            .await
            .with_context(|| format!("绑定 VPN DNS TCP {address} 失败"))?;
        tracing::info!(%address, "VPN 内置 DNS 已启动");
        tokio::select! {
            result = self.clone().run_udp(udp) => result,
            result = self.run_tcp(tcp) => result,
        }
    }

    async fn run_udp(self, socket: UdpSocket) -> anyhow::Result<()> {
        let socket = Arc::new(socket);
        let mut buffer = vec![0_u8; MAX_DNS_PACKET];
        loop {
            let (length, source) = socket.recv_from(&mut buffer).await?;
            if !self.source_allowed(source) {
                tracing::warn!(source = %source.ip(), "拒绝非 VPN 来源的 DNS UDP 请求");
                continue;
            }
            let request = buffer[..length].to_vec();
            let Ok(permit) = self.concurrency.clone().try_acquire_owned() else {
                tracing::warn!("DNS 并发请求已达上限，丢弃 UDP 请求");
                continue;
            };
            let server = self.clone();
            let response_socket = socket.clone();
            tokio::spawn(async move {
                let _permit = permit;
                let response = server.process(&request).await;
                if let Err(error) = response_socket.send_to(&response, source).await {
                    tracing::warn!(%error, "发送 DNS UDP 响应失败");
                }
            });
        }
    }

    async fn run_tcp(self, listener: TcpListener) -> anyhow::Result<()> {
        loop {
            let (stream, source) = listener.accept().await?;
            if !self.source_allowed(source) {
                tracing::warn!(source = %source.ip(), "拒绝非 VPN 来源的 DNS TCP 请求");
                continue;
            }
            let Ok(permit) = self.concurrency.clone().try_acquire_owned() else {
                tracing::warn!("DNS 并发请求已达上限，拒绝 TCP 请求");
                continue;
            };
            let server = self.clone();
            tokio::spawn(async move {
                let _permit = permit;
                if let Err(error) = server.handle_tcp_client(stream).await {
                    tracing::warn!(%error, "处理 DNS TCP 请求失败");
                }
            });
        }
    }

    fn source_allowed(&self, source: SocketAddr) -> bool {
        matches!(source.ip(), std::net::IpAddr::V4(ip) if self.subnet.contains(&ip))
    }

    async fn handle_tcp_client(&self, mut stream: TcpStream) -> anyhow::Result<()> {
        loop {
            let length = match timeout(TCP_CLIENT_TIMEOUT, stream.read_u16()).await {
                Ok(Ok(length)) => usize::from(length),
                Ok(Err(error)) if error.kind() == std::io::ErrorKind::UnexpectedEof => {
                    return Ok(())
                }
                Ok(Err(error)) => return Err(error.into()),
                Err(_) => return Ok(()),
            };
            if length == 0 {
                return Err(anyhow!("DNS TCP 帧长度为 0"));
            }
            let mut request = vec![0_u8; length];
            timeout(TCP_CLIENT_TIMEOUT, stream.read_exact(&mut request))
                .await
                .context("DNS TCP 客户端读取超时")??;
            let response = self.process(&request).await;
            let response_length = u16::try_from(response.len()).context("DNS TCP 响应过大")?;
            timeout(TCP_CLIENT_TIMEOUT, async {
                stream.write_u16(response_length).await?;
                stream.write_all(&response).await
            })
            .await
            .context("DNS TCP 客户端写入超时")??;
        }
    }

    async fn process(&self, raw: &[u8]) -> Vec<u8> {
        let request = match Message::from_vec(raw) {
            Ok(message) => message,
            Err(_) => return raw_error_id(raw, OpCode::Query, ResponseCode::FormErr),
        };
        if request.message_type != MessageType::Query
            || request.op_code != OpCode::Query
            || request.queries.len() != 1
        {
            return error_response(&request, ResponseCode::FormErr);
        }
        let settings = self.settings.read().await.clone();
        if settings.mode == ClientDnsMode::Disabled {
            return error_response(&request, ResponseCode::Refused);
        }
        let query = &request.queries[0];
        let name = query
            .name()
            .to_ascii()
            .trim_end_matches('.')
            .to_ascii_lowercase();

        if query.query_type() == RecordType::A {
            if let Some(record) = settings.static_records.iter().find(|record| {
                normalize_dns_domain(&record.name).is_ok_and(|candidate| candidate == name)
            }) {
                return static_response(&request, record.address, record.ttl);
            }
        }

        let upstreams = select_upstreams(&settings, &name);
        if upstreams.is_empty() {
            return error_response(&request, ResponseCode::ServFail);
        }
        // ID 之外保留完整请求作为缓存键，避免把不同 DNSSEC/EDNS/CD/RD 语义混用。
        let mut cache_request = raw.to_vec();
        if cache_request.len() >= 2 {
            cache_request[..2].fill(0);
        }
        let key = CacheKey {
            request: cache_request,
            upstreams: upstreams.join(","),
        };
        if let Some(packet) = self.cache.lock().await.get(&key, request.id) {
            tracing::debug!(record_type = ?query.query_type(), result = "cache_hit", "DNS 查询完成");
            return packet;
        }

        for upstream in upstreams {
            let Ok(address) = upstream.parse::<SocketAddr>() else {
                continue;
            };
            match query_upstream(raw, address).await {
                Ok(packet) => {
                    let Ok(response) = Message::from_vec(&packet) else {
                        continue;
                    };
                    if response.id != request.id
                        || response.message_type != MessageType::Response
                        || response.queries != request.queries
                    {
                        continue;
                    }
                    if let Some(ttl) = cache_ttl(&response) {
                        self.cache
                            .lock()
                            .await
                            .insert(key.clone(), packet.clone(), ttl);
                    }
                    tracing::debug!(record_type = ?query.query_type(), result = "forwarded", "DNS 查询完成");
                    return packet;
                }
                Err(error) => {
                    tracing::warn!(%error, upstream = %address, "DNS 上游查询失败，尝试下一个")
                }
            }
        }
        error_response(&request, ResponseCode::ServFail)
    }
}

fn select_upstreams(settings: &DnsNetworkSettings, name: &str) -> Vec<String> {
    settings
        .forward_rules
        .iter()
        .filter_map(|rule| {
            let domain = normalize_dns_domain(&rule.domain).ok()?;
            (name == domain || name.ends_with(&format!(".{domain}")))
                .then_some((domain.len(), rule.upstreams.clone()))
        })
        .max_by_key(|(length, _)| *length)
        .map(|(_, upstreams)| upstreams)
        .unwrap_or_else(|| settings.default_upstreams.clone())
}

fn static_response(request: &Message, address: Ipv4Addr, ttl: u32) -> Vec<u8> {
    let query = request.queries[0].clone();
    let mut response = Message::response(request.id, request.op_code);
    response.metadata.recursion_desired = request.recursion_desired;
    response.metadata.recursion_available = true;
    response.metadata.authoritative = true;
    response.edns = request.edns.clone();
    response.add_query(query.clone());
    response.add_answer(Record::from_rdata(
        query.name().clone(),
        ttl,
        RData::A(A(address)),
    ));
    response
        .to_vec()
        .unwrap_or_else(|_| error_response(request, ResponseCode::ServFail))
}

fn error_response(request: &Message, code: ResponseCode) -> Vec<u8> {
    let mut response = Message::error_msg(request.id, request.op_code, code);
    response.metadata.recursion_desired = request.recursion_desired;
    response.metadata.recursion_available = true;
    response.edns = request.edns.clone();
    response.add_queries(request.queries.clone());
    response
        .to_vec()
        .unwrap_or_else(|_| raw_error_id(&request.id.to_be_bytes(), request.op_code, code))
}

fn raw_error_id(raw: &[u8], op_code: OpCode, code: ResponseCode) -> Vec<u8> {
    let id = raw
        .get(..2)
        .and_then(|bytes| <[u8; 2]>::try_from(bytes).ok())
        .map(u16::from_be_bytes)
        .unwrap_or_default();
    Message::error_msg(id, op_code, code)
        .to_vec()
        .unwrap_or_default()
}

fn cache_ttl(response: &Message) -> Option<Duration> {
    if !matches!(
        response.response_code,
        ResponseCode::NoError | ResponseCode::NXDomain
    ) {
        return None;
    }
    let negative = response.response_code == ResponseCode::NXDomain || response.answers.is_empty();
    let seconds = if negative {
        response
            .authorities
            .iter()
            .find_map(|record| match &record.data {
                RData::SOA(soa) => Some(record.ttl.min(soa.minimum)),
                _ => None,
            })
            .unwrap_or(30)
            .min(300)
    } else {
        response
            .answers
            .iter()
            .map(|record| record.ttl)
            .min()
            .unwrap_or(30)
            .min(3_600)
    }
    .max(1);
    Some(Duration::from_secs(u64::from(seconds)))
}

async fn query_upstream(request: &[u8], upstream: SocketAddr) -> anyhow::Result<Vec<u8>> {
    let socket = UdpSocket::bind("0.0.0.0:0").await?;
    socket.connect(upstream).await?;
    socket.send(request).await?;
    let mut buffer = vec![0_u8; MAX_DNS_PACKET];
    let length = timeout(UPSTREAM_TIMEOUT, socket.recv(&mut buffer))
        .await
        .context("DNS UDP 上游超时")??;
    buffer.truncate(length);
    let response = Message::from_vec(&buffer).context("DNS UDP 上游返回非法报文")?;
    if response.truncation {
        return query_upstream_tcp(request, upstream).await;
    }
    Ok(buffer)
}

async fn query_upstream_tcp(request: &[u8], upstream: SocketAddr) -> anyhow::Result<Vec<u8>> {
    let mut stream = timeout(UPSTREAM_TIMEOUT, TcpStream::connect(upstream))
        .await
        .context("DNS TCP 上游连接超时")??;
    let length = u16::try_from(request.len()).context("DNS 请求过大")?;
    timeout(UPSTREAM_TIMEOUT, async {
        stream.write_u16(length).await?;
        stream.write_all(request).await?;
        let response_length = usize::from(stream.read_u16().await?);
        let mut response = vec![0_u8; response_length];
        stream.read_exact(&mut response).await?;
        Ok::<_, std::io::Error>(response)
    })
    .await
    .context("DNS TCP 上游读写超时")?
    .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hickory_proto::{op::Query, rr::Name};
    use std::str::FromStr;
    use vpn_api_types::system::{DnsForwardRule, DnsStaticRecord};

    fn query(id: u16, name: &str) -> Vec<u8> {
        let mut message = Message::query();
        message.metadata.id = id;
        message.add_query(Query::query(Name::from_str(name).unwrap(), RecordType::A));
        message.to_vec().unwrap()
    }

    fn answer(request: &[u8], address: Ipv4Addr, truncated: bool) -> Vec<u8> {
        let request = Message::from_vec(request).unwrap();
        let query = request.queries[0].clone();
        let mut response = Message::response(request.id, request.op_code);
        response.metadata.truncation = truncated;
        response.add_query(query.clone());
        if !truncated {
            response.add_answer(Record::from_rdata(
                query.name().clone(),
                60,
                RData::A(A(address)),
            ));
        }
        response.to_vec().unwrap()
    }

    #[test]
    fn longest_suffix_wins() {
        let settings = DnsNetworkSettings {
            mode: ClientDnsMode::Split,
            split_domains: vec!["corp.example.com".into()],
            default_upstreams: vec!["1.1.1.1:53".into()],
            forward_rules: vec![
                DnsForwardRule {
                    domain: "example.com".into(),
                    upstreams: vec!["2.2.2.2:53".into()],
                },
                DnsForwardRule {
                    domain: "corp.example.com".into(),
                    upstreams: vec!["3.3.3.3:53".into()],
                },
            ],
            static_records: vec![],
        };
        assert_eq!(
            select_upstreams(&settings, "api.corp.example.com"),
            vec!["3.3.3.3:53"]
        );
        assert_eq!(
            select_upstreams(&settings, "other.test"),
            vec!["1.1.1.1:53"]
        );
    }

    #[tokio::test]
    async fn static_record_precedes_forwarding_and_disabled_refuses() {
        let settings = Arc::new(RwLock::new(DnsNetworkSettings {
            mode: ClientDnsMode::Global,
            default_upstreams: vec!["127.0.0.1:53".into()],
            static_records: vec![DnsStaticRecord {
                name: "api.internal.example.com".into(),
                address: "10.20.30.40".parse().unwrap(),
                ttl: 300,
            }],
            ..Default::default()
        }));
        let server = DnsServer::new("10.9.0.0/24".parse().unwrap(), settings.clone()).unwrap();
        let response =
            Message::from_vec(&server.process(&query(7, "api.internal.example.com")).await)
                .unwrap();
        assert_eq!(response.id, 7);
        assert!(
            matches!(&response.answers[0].data, RData::A(A(ip)) if *ip == "10.20.30.40".parse::<Ipv4Addr>().unwrap())
        );

        settings.write().await.mode = ClientDnsMode::Disabled;
        let refused =
            Message::from_vec(&server.process(&query(8, "api.internal.example.com")).await)
                .unwrap();
        assert_eq!(refused.response_code, ResponseCode::Refused);
        assert!(server.source_allowed("10.9.0.2:53000".parse().unwrap()));
        assert!(!server.source_allowed("192.168.1.2:53000".parse().unwrap()));
    }

    #[tokio::test]
    async fn forwarded_answer_is_cached_and_id_is_rewritten() {
        let upstream = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let address = upstream.local_addr().unwrap();
        let worker = tokio::spawn(async move {
            let mut buffer = [0_u8; 512];
            let (length, source) = upstream.recv_from(&mut buffer).await.unwrap();
            let response = answer(&buffer[..length], "10.1.2.3".parse().unwrap(), false);
            upstream.send_to(&response, source).await.unwrap();
        });
        let settings = Arc::new(RwLock::new(DnsNetworkSettings {
            mode: ClientDnsMode::Global,
            default_upstreams: vec![address.to_string()],
            ..Default::default()
        }));
        let server = DnsServer::new("10.9.0.0/24".parse().unwrap(), settings).unwrap();
        let first =
            Message::from_vec(&server.process(&query(10, "cache.example.com")).await).unwrap();
        let second =
            Message::from_vec(&server.process(&query(11, "cache.example.com")).await).unwrap();
        worker.await.unwrap();
        assert_eq!(first.id, 10);
        assert_eq!(second.id, 11);
        assert_eq!(server.cache.lock().await.entries.len(), 1);
    }

    #[tokio::test]
    async fn truncated_udp_response_falls_back_to_tcp() {
        let tcp = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = tcp.local_addr().unwrap();
        let udp = UdpSocket::bind(address).await.unwrap();
        let udp_worker = tokio::spawn(async move {
            let mut buffer = [0_u8; 512];
            let (length, source) = udp.recv_from(&mut buffer).await.unwrap();
            let response = answer(&buffer[..length], "10.4.5.6".parse().unwrap(), true);
            udp.send_to(&response, source).await.unwrap();
        });
        let tcp_worker = tokio::spawn(async move {
            let (mut stream, _) = tcp.accept().await.unwrap();
            let length = usize::from(stream.read_u16().await.unwrap());
            let mut request = vec![0_u8; length];
            stream.read_exact(&mut request).await.unwrap();
            let response = answer(&request, "10.4.5.6".parse().unwrap(), false);
            stream.write_u16(response.len() as u16).await.unwrap();
            stream.write_all(&response).await.unwrap();
        });
        let response = query_upstream(&query(12, "tcp.example.com"), address)
            .await
            .unwrap();
        let response = Message::from_vec(&response).unwrap();
        udp_worker.await.unwrap();
        tcp_worker.await.unwrap();
        assert!(!response.truncation);
        assert_eq!(response.answers.len(), 1);
    }
}
