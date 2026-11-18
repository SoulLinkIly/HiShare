use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

use rand::RngCore;
use serde_json::Value;

use crate::{Error, Result};

pub const PORT: u16 = 5684;

pub const MULTICAST: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 1);

static SCAN_NOW: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn request_scan() {
    SCAN_NOW.store(true, std::sync::atomic::Ordering::Relaxed);
}

pub fn take_scan_request() -> bool {
    SCAN_NOW.swap(false, std::sync::atomic::Ordering::Relaxed)
}
pub const SHARE_CAPABILITY_BIT: u64 = 256;

#[derive(Debug, Clone)]
pub struct Peer {
    pub id: String,
    pub host: String,
    pub port: u16,
    pub name: String,
    pub brand: String,
    pub kind: String,
    pub last_seen: Instant,

    pub accepts: bool,
}

pub fn is_discovery_packet(data: &[u8]) -> bool {
    data.len() >= 32 && data.windows(15).any(|w| w == b"device_discover")
}

pub fn discovery_beacon(local_ip: &str, port: u16, identity: &str, name: &str, discovering: bool) -> Vec<u8> {

    let seq_no = (rand::random::<u16>()) as u32;
    let device_id = format!(r#"{{"UDID":"{identity}"}}"#);
    let mode = if discovering { 1 } else { 10 };

    let mut json = if discovering {
        format!(
            concat!(
                r#"{{"deviceId":{},"devicename":{},"type":{},"hicomversion":"","mode":{},"#,
                r#""deviceHash":"","serviceData":"port:{},","extendServiceData":"summary:{},nbVer:9,OH:1,icon:1,abl:31,rssi:-58","#,
                r#""wlanIp":"{}","capabilityBitmap":[{}],"bType":5,"seqNo":{}"#
            ),
            json_string(&device_id),
            json_string(crate::clip_chars(name, 60)),
            DEVICE_TYPE_PC,
            mode,
            port,
            summary_hex(identity),
            local_ip,
            CAPABILITY_BITS,
            seq_no
        )
    } else {
        format!(
            concat!(
                r#"{{"deviceId":{},"devicename":{},"type":{},"hicomversion":"","mode":{},"#,
                r#""deviceHash":"","serviceData":"port:{},","extendServiceData":"osType:2,nbVer:9,rssi:-58,abl:8","#,
                r#""wlanIp":"{}","capabilityBitmap":[{}],"bType":5,"seqNo":{}"#
            ),
            json_string(&device_id),
            json_string(crate::clip_chars(name, 60)),
            DEVICE_TYPE_PC,
            mode,
            port,
            local_ip,
            CAPABILITY_BITS,
            seq_no
        )
    };
    json.push('}');

    let mut out = Vec::with_capacity(32 + json.len());
    if discovering {
        out.extend_from_slice(&[0x50, 0x02]);
        out.extend_from_slice(&(rand::random::<u16>()).to_be_bytes());
        push_option(&mut out, 3, local_ip.as_bytes());
        push_option(&mut out, 8, b"device_discover");
        out.push(0xFF);
    } else {
        let mut token = [0u8; 4];
        rand::rngs::OsRng.fill_bytes(&mut token);
        out.extend_from_slice(&[0x42, 0x02]);
        out.extend_from_slice(&token);
        out.extend_from_slice(&[0xBD, 0x02]);
        out.extend_from_slice(b"device_discover");
        out.extend_from_slice(&[0x11, 0x32]);
        out.push(0xFF);
    }
    out.extend_from_slice(json.as_bytes());
    crate::trace::discovery_build(local_ip, port, name, DEVICE_TYPE_PC);
    out
}

fn push_option(out: &mut Vec<u8>, delta: u8, value: &[u8]) {
    let len = value.len();
    if len < 13 {
        out.push((delta << 4) | len as u8);
    } else {
        out.push((delta << 4) | 13);
        out.push((len - 13) as u8);
    }
    out.extend_from_slice(value);
}

pub fn advertisement_query_harmony(local_ip: &str, port: u16, identity: &str, name: &str) -> Vec<u8> {
    let mut token = [0u8; 4];
    rand::rngs::OsRng.fill_bytes(&mut token);
    let seq_no = (rand::random::<u16>()) as u32;
    let device_id = format!(r#"{{"UDID":"{identity}"}}"#);
    let json = format!(
        concat!(
            r#"{{"deviceId":{},"devicename":{},"type":{},"hicomversion":"","mode":1,"#,
            r#""deviceHash":"","serviceData":"port:{},","extendServiceData":"osType:2,nbVer:9,rssi:-58,abl:8","#,
            r#""wlanIp":"{}","capabilityBitmap":[{}],"bType":5,"seqNo":{},"#,
            r#""coapUri":"coap://{}/device_discover""#
        ),
        json_string(&device_id),
        json_string(crate::clip_chars(name, 60)),
        DEVICE_TYPE_PC,
        port,
        local_ip,
        CAPABILITY_BITS,
        seq_no,
        local_ip
    ) + "}";

    let mut out = Vec::with_capacity(32 + json.len());
    out.extend_from_slice(&[0x42, 0x02]);
    out.extend_from_slice(&token);
    out.extend_from_slice(&[0xBD, 0x02]);
    out.extend_from_slice(b"device_discover");
    out.extend_from_slice(&[0x11, 0x32]);
    out.push(0xFF);
    out.extend_from_slice(json.as_bytes());
    out
}

fn summary_hex(identity: &str) -> String {
    use sha1::{Digest, Sha1};
    let mut hasher = Sha1::new();
    hasher.update(identity.as_bytes());
    hasher.finalize().iter().map(|b| format!("{b:02X}")).collect()
}

const CAPABILITY_BITS: u64 = 455;

const DEVICE_TYPE_PC: u32 = 12;

fn json_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn parse_options(data: &[u8]) -> Result<(usize, HashMap<u64, Vec<u8>>)> {
    let mut offset = 0usize;
    let mut number = 0u64;
    let mut options: HashMap<u64, Vec<u8>> = HashMap::new();

    let read_extended = |nibble: u8, offset: &mut usize| -> Result<u64> {
        match nibble {
            n if n < 13 => Ok(n as u64),
            13 => {
                if *offset >= data.len() {
                    return Err(Error::Truncated);
                }
                let v = data[*offset] as u64 + 13;
                *offset += 1;
                Ok(v)
            }
            14 => {
                if *offset + 2 > data.len() {
                    return Err(Error::Truncated);
                }
                let v = u16::from_be_bytes([data[*offset], data[*offset + 1]]) as u64 + 269;
                *offset += 2;
                Ok(v)
            }
            _ => Err(Error::Format),
        }
    };

    while offset < data.len() && data[offset] != 0xFF {
        let initial = data[offset];
        offset += 1;
        number += read_extended(initial >> 4, &mut offset)?;
        let length = read_extended(initial & 0x0F, &mut offset)? as usize;
        if offset + length > data.len() || options.len() >= 16 {
            return Err(Error::Truncated);
        }
        options.insert(number, data[offset..offset + length].to_vec());
        offset += length;
    }
    Ok((offset, options))
}

pub fn discovery_ack(message_id: [u8; 2]) -> [u8; 4] {
    [0x60, 0x00, message_id[0], message_id[1]]
}

pub fn parse_discovery_beacon(
    data: &[u8],
    host: &str,
    lans: &[crate::net::Nic],
) -> Result<(Peer, bool, [u8; 2])> {
    let src: Ipv4Addr = host.parse().map_err(|_| Error::Format)?;
    if lans.iter().any(|nic| nic.ip == src) {
        return Err(Error::Protocol("本机自己的广播".into()));
    }
    if !lans.iter().any(|nic| nic.contains(src)) {
        return Err(Error::Protocol("源地址不在本网段".into()));
    }
    if data.len() < 4 || data[0] >> 6 != 1 || (data[0] & 0x0F) > 8 {
        return Err(Error::Format);
    }
    let message_type = (data[0] >> 4) & 3;
    let token_size = (data[0] & 0x0F) as usize;
    if message_type > 1 || data[1] != 2 || 4 + token_size > data.len() {
        return Err(Error::Format);
    }

    let mut offset = 4 + token_size;
    let (payload_marker, options) = parse_options(&data[offset..])?;
    offset += payload_marker;

    match options.get(&11) {
        Some(v) if v == b"device_discover" => {}
        _ => return Err(Error::Format),
    }
    if offset >= data.len() {
        return Err(Error::Truncated);
    }
    let mut payload = &data[offset + 1..];
    if payload.last() == Some(&0) {
        payload = &payload[..payload.len() - 1];
    }

    let value: Value = serde_json::from_slice(payload).map_err(|_| Error::Format)?;
    let obj = value.as_object().ok_or(Error::Format)?;

    if obj.get("wlanIp").and_then(|v| v.as_str()) != Some(host) {
        return Err(Error::Protocol("广播地址与源 IP 不一致".into()));
    }
    if obj.get("bType").and_then(|v| v.as_u64()) != Some(5) {
        return Err(Error::Format);
    }

    let caps = obj
        .get("capabilityBitmap")
        .and_then(|v| v.as_array())
        .ok_or(Error::Format)?;
    if caps.is_empty() || caps.len() > 4 {
        return Err(Error::Format);
    }
    let first_cap = caps[0].as_u64().unwrap_or(0);

    let kind = match obj.get("type").and_then(|v| v.as_u64()) {
        Some(12) => "computer",
        Some(14) => "phone",
        Some(17) => "tablet",
        _ => "unknown",
    };

    let accepts = first_cap & SHARE_CAPABILITY_BIT != 0;

    let service = obj.get("serviceData").and_then(|v| v.as_str()).unwrap_or("");
    let port = service
        .split(',')
        .find_map(|item| item.trim().strip_prefix("port:"))
        .and_then(|s| s.parse::<u16>().ok())
        .ok_or(Error::Format)?;

    let name = obj.get("devicename").and_then(|v| v.as_str()).unwrap_or("Huawei").to_string();

    let device_id = obj
        .get("deviceId")
        .and_then(|v| v.as_str())
        .and_then(|s| serde_json::from_str::<Value>(s).ok())
        .and_then(|v| v.get("UDID").and_then(|u| u.as_str()).map(|s| s.trim().to_string()))
        .unwrap_or_default();

    let peer = Peer {
        id: device_id,
        host: host.to_string(),
        port,
        name,
        brand: "Huawei".to_string(),
        kind: kind.to_string(),
        last_seen: Instant::now(),
        accepts,
    };

    let confirmable = message_type == 0;
    let message_id = [data[2], data[3]];
    crate::trace::discovery_rx(&peer);
    Ok((peer, confirmable, message_id))
}

fn unicast_target(target: &str) -> Result<Ipv4Addr> {
    target.parse().map_err(|_| Error::Argument)
}

pub struct DiscoverySocket {
    socket: UdpSocket,
}

impl DiscoverySocket {
    pub fn bind() -> Result<Self> {
        let socket = UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), PORT))
            .map_err(|e| Error::Io(format!("绑定 0.0.0.0:{PORT} 失败: {e}")))?;
        socket
            .set_broadcast(true)
            .map_err(|e| Error::Io(format!("开启广播失败: {e}")))?;
        socket
            .set_read_timeout(Some(Duration::from_millis(400)))
            .map_err(|e| Error::Io(e.to_string()))?;
        let _ = socket.join_multicast_v4(&MULTICAST, &Ipv4Addr::UNSPECIFIED);
        Ok(Self { socket })
    }

    pub fn send(&self, dst: Ipv4Addr, data: &[u8]) -> Result<()> {
        self.socket
            .send_to(data, SocketAddr::new(IpAddr::V4(dst), PORT))
            .map_err(|e| Error::Io(format!("发送到 {dst} 失败: {e}")))?;
        crate::trace::discovery_tx(&dst.to_string(), data);
        Ok(())
    }

    pub fn transmit(&self, target: &str, data: &[u8]) -> Result<()> {
        self.send(unicast_target(target)?, data)
    }

    pub fn recv(&self) -> Option<(String, Vec<u8>)> {
        let mut buf = vec![0u8; 8192];
        match self.socket.recv_from(&mut buf) {
            Ok((n, addr)) => {
                buf.truncate(n);
                crate::trace::discovery_rx_raw(&addr.ip().to_string(), &buf);
                Some((addr.ip().to_string(), buf))
            }
            Err(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keepalive_packet_is_not_discovery() {

        assert!(!is_discovery_packet(&[0x60, 0x00, 0xF5, 0x8E]));
        assert!(!is_discovery_packet(&[]));

        let pkt = discovery_beacon("192.168.1.5", 41234, "AABB", "PC", true);
        assert!(is_discovery_packet(&pkt));
        let answer = discovery_beacon("192.168.1.5", 41234, "AABB", "PC", false);
        assert!(is_discovery_packet(&answer));

        assert!(!is_discovery_packet(&vec![0u8; 100]));
    }

    #[test]
    fn advertisement_shape() {

        let query = discovery_beacon("192.168.1.5", 41234, "AABBCCDDEEFF", "我的电脑", true);
        assert_eq!(&query[0..2], &[0x50, 0x02]);
        assert!(query.windows(15).any(|w| w == b"device_discover"));
        assert_eq!(*query.last().unwrap(), b'}');
        let text = String::from_utf8_lossy(&query);
        assert!(text.contains(r#""wlanIp":"192.168.1.5""#));
        assert!(text.contains(r#""mode":1"#));
        assert!(text.contains("summary:"));

        assert!(text.contains(r#""type":12"#));

        let answer = discovery_beacon("192.168.1.5", 41234, "AABBCCDDEEFF", "我的电脑", false);
        assert_eq!(&answer[0..2], &[0x42, 0x02]);
        let text = String::from_utf8_lossy(&answer);
        assert!(text.contains(r#""mode":10"#));
        assert!(text.contains("osType:2"));
    }

    #[test]
    fn json_escaping() {
        assert_eq!(json_string(r#"a"b"#), r#""a\"b""#);
    }

    #[test]
    fn ack_frame_shape() {

        let ack = discovery_ack([0xD8, 0x3C]);
        assert_eq!(ack, [0x60, 0x00, 0xD8, 0x3C]);
        assert_eq!((ack[0] >> 4) & 3, 2);
        assert_eq!(ack[1], 0x00);
    }

    fn lans() -> Vec<crate::net::Nic> {
        let row = |ip: &str, mask: &str| -> (u32, u32) {
            let ip: Ipv4Addr = ip.parse().unwrap();
            let mask: Ipv4Addr = mask.parse().unwrap();
            (
                u32::from_ne_bytes(ip.octets()),
                u32::from_ne_bytes(mask.octets()),
            )
        };
        crate::net::nics_from_rows(&[
            row("192.168.123.3", "255.255.255.0"),
            row("10.8.3.4", "255.255.0.0"),
        ])
    }

    #[test]
    fn transmit_takes_only_a_real_ip() {
        assert_eq!(
            unicast_target("192.168.123.7").unwrap(),
            Ipv4Addr::new(192, 168, 123, 7)
        );
        assert!(unicast_target("broadcast").is_err(), "不许再走全局广播");
        assert!(unicast_target("multicast").is_err(), "不许再走组播发送");
    }

    #[test]
    fn phone_on_either_nic_is_accepted() {
        for ip in ["192.168.123.7", "10.8.9.7"] {
            let pkt = discovery_beacon(ip, 41234, "AABB", "nova", false);
            let got = parse_discovery_beacon(&pkt, ip, &lans());
            assert!(got.is_ok(), "{ip} 应当被接受：{got:?}");
        }
    }

    #[test]
    fn own_broadcast_is_dropped() {
        let ip = "192.168.123.3";
        let pkt = discovery_beacon(ip, 41234, "AABB", "PC", true);
        let err = parse_discovery_beacon(&pkt, ip, &lans()).unwrap_err();
        assert!(err.to_string().contains("本机"), "自己的广播会被回环收到，必须丢：{err}");
    }

    #[test]
    fn foreign_subnet_is_dropped() {
        let ip = "172.16.0.9";
        let pkt = discovery_beacon(ip, 41234, "AABB", "nova", false);
        let err = parse_discovery_beacon(&pkt, ip, &lans()).unwrap_err();
        assert!(err.to_string().contains("网段"), "{err}");
    }
}
