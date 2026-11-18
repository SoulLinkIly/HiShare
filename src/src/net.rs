use std::net::Ipv4Addr;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Nic {
    pub ip: Ipv4Addr,
    pub mask: Ipv4Addr,
    pub broadcast: Ipv4Addr,
}

const MAX_NICS: usize = 16;

impl Nic {
    pub fn network(&self) -> u32 {
        u32::from(self.ip) & u32::from(self.mask)
    }

    pub fn contains(&self, ip: Ipv4Addr) -> bool {
        let mask = u32::from(self.mask);
        mask != 0 && (u32::from(ip) & mask) == self.network()
    }
}

pub fn nic_from_row(addr: u32, mask: u32) -> Option<Nic> {
    let ip = Ipv4Addr::from(addr.to_ne_bytes());
    let mask = Ipv4Addr::from(mask.to_ne_bytes());
    let bits = u32::from(mask);
    if bits == 0 || bits == u32::MAX {
        return None;
    }
    if ip.is_unspecified() || ip.is_loopback() || ip.is_link_local() {
        return None;
    }
    Some(Nic {
        ip,
        mask,
        broadcast: Ipv4Addr::from(u32::from(ip) | !bits),
    })
}

pub fn nics_from_rows(rows: &[(u32, u32)]) -> Vec<Nic> {
    let mut out: Vec<Nic> = Vec::new();
    for (addr, mask) in rows {
        let Some(nic) = nic_from_row(*addr, *mask) else {
            continue;
        };
        if out.iter().any(|seen| seen.network() == nic.network()) {
            continue;
        }
        out.push(nic);
        if out.len() >= MAX_NICS {
            break;
        }
    }
    out
}

#[cfg(windows)]
pub fn nics() -> Vec<Nic> {
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        GetIpAddrTable, MIB_IPADDRROW_XP, MIB_IPADDRTABLE,
    };

    let mut size = 0u32;
    unsafe {
        GetIpAddrTable(std::ptr::null_mut(), &mut size, 0);
    }
    if size < std::mem::size_of::<MIB_IPADDRTABLE>() as u32 {
        return Vec::new();
    }
    let mut buf = vec![0u32; (size as usize + 3) / 4];
    let table = buf.as_mut_ptr().cast::<MIB_IPADDRTABLE>();
    let rows: Vec<(u32, u32)> = unsafe {
        if GetIpAddrTable(table, &mut size, 0) != 0 {
            return Vec::new();
        }
        let count = (*table).dwNumEntries as usize;
        std::slice::from_raw_parts(
            std::ptr::addr_of!((*table).table).cast::<MIB_IPADDRROW_XP>(),
            count,
        )
        .iter()
        .map(|row| (row.dwAddr, row.dwMask))
        .collect()
    };
    nics_from_rows(&rows)
}

#[cfg(not(windows))]
pub fn nics() -> Vec<Nic> {
    Vec::new()
}

pub fn lans() -> Vec<Nic> {
    nics()
}

pub fn allows(ip: Ipv4Addr) -> bool {
    if ip.is_loopback() {
        return true;
    }
    nics().iter().any(|nic| nic.contains(ip))
}

pub fn text() -> String {
    let list = lans();
    if list.is_empty() {
        return "无可用网卡".to_string();
    }
    list.iter()
        .map(|nic| nic.ip.to_string())
        .collect::<Vec<_>>()
        .join("、")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(ip: &str, mask: &str) -> (u32, u32) {
        let addr = ip.parse::<Ipv4Addr>().unwrap();
        let mask = mask.parse::<Ipv4Addr>().unwrap();
        (
            u32::from_ne_bytes(addr.octets()),
            u32::from_ne_bytes(mask.octets()),
        )
    }

    #[test]
    fn row_is_matched_in_network_byte_order() {
        let (addr, mask) = row("192.168.123.3", "255.255.255.0");
        let nic = nic_from_row(addr, mask).expect("应当解析出一块网卡");
        assert_eq!(nic.ip, Ipv4Addr::new(192, 168, 123, 3));
        assert_eq!(nic.broadcast, Ipv4Addr::new(192, 168, 123, 255));
        assert_eq!(
            nic_from_row(u32::from_ne_bytes([192, 168, 123, 4]), mask).map(|n| n.ip),
            Some(Ipv4Addr::new(192, 168, 123, 4)),
            "IP 表里是网络字节序，按主机序解会得到倒过来的地址"
        );
    }

    #[test]
    fn unusable_rows_are_dropped() {
        assert!(nic_from_row(u32::from_ne_bytes([127, 0, 0, 1]), u32::from_ne_bytes([255, 0, 0, 0])).is_none(), "回环");
        assert!(nic_from_row(u32::from_ne_bytes([169, 254, 3, 9]), u32::from_ne_bytes([255, 255, 0, 0])).is_none(), "自动私有地址");
        assert!(nic_from_row(u32::from_ne_bytes([10, 0, 0, 5]), 0).is_none(), "掩码为 0");
        assert!(nic_from_row(u32::from_ne_bytes([10, 0, 0, 5]), u32::MAX).is_none(), "掩码全 1");
    }

    #[test]
    fn table_keeps_one_entry_per_subnet() {
        let rows = vec![
            row("192.168.123.3", "255.255.255.0"),
            row("192.168.123.9", "255.255.255.0"),
            row("10.8.0.2", "255.255.0.0"),
            row("169.254.1.1", "255.255.0.0"),
        ];
        let list = nics_from_rows(&rows);
        assert_eq!(list.len(), 2, "同子网只留一条，链路本地丢掉：{list:?}");
        assert_eq!(list[0].ip, Ipv4Addr::new(192, 168, 123, 3));
        assert_eq!(list[1].broadcast, Ipv4Addr::new(10, 8, 255, 255));
    }

    #[test]
    fn contains_uses_the_real_mask() {
        let (addr, mask) = row("10.8.3.4", "255.255.0.0");
        let nic = nic_from_row(addr, mask).unwrap();
        assert!(nic.contains("10.8.99.7".parse().unwrap()));
        assert!(!nic.contains("10.9.0.1".parse().unwrap()));
        assert!(!nic.contains("192.168.123.7".parse().unwrap()));
    }

    #[test]
    fn mixed_subnets_are_all_recognized() {
        let rows = vec![
            row("192.168.123.3", "255.255.255.0"),
            row("10.8.3.4", "255.255.0.0"),
        ];
        let list = nics_from_rows(&rows);
        let inside = |ip: &str| list.iter().any(|nic| nic.contains(ip.parse().unwrap()));
        assert!(inside("192.168.123.7"), "Wi-Fi 上的手机");
        assert!(inside("10.8.9.9"), "网线上的手机");
        assert!(!inside("172.16.0.2"), "别的网段不许进");
    }

    #[test]
    fn nic_count_is_capped_to_keep_broadcast_bounded() {
        let rows: Vec<(u32, u32)> = (1..=20u8)
            .map(|i| row(&format!("10.{i}.0.1"), "255.255.0.0"))
            .collect();
        assert_eq!(nics_from_rows(&rows).len(), MAX_NICS);
    }
}
