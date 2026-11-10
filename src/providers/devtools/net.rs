//! 本机 IP（`getifaddrs`，不联网）与公网 IP（只在用户回车时请求）。

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::time::Duration;

use anyhow::{Context as _, Result};

use crate::i18n::{tr, trf};

const DEFAULT_PUBLIC_IP_URL: &str = "https://api.ipify.org";

/// 本机网卡上的地址：(网卡名, 地址)。跳过回环和 IPv6 链路本地地址，IPv4 在前。
pub fn local_addresses() -> Vec<(String, IpAddr)> {
    let mut out = Vec::new();
    let mut head: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: getifaddrs 成功时把链表头写进 head，用完由 freeifaddrs 释放；遍历期间只读。
    unsafe {
        if libc::getifaddrs(&mut head) != 0 {
            return out;
        }
        let mut cursor = head;
        while let Some(entry) = cursor.as_ref() {
            cursor = entry.ifa_next;
            let Some(addr) = entry.ifa_addr.as_ref() else {
                continue;
            };
            let name = std::ffi::CStr::from_ptr(entry.ifa_name)
                .to_string_lossy()
                .into_owned();
            let ip = match i32::from(addr.sa_family) {
                libc::AF_INET => {
                    let v4 = &*(entry.ifa_addr as *const libc::sockaddr_in);
                    IpAddr::V4(Ipv4Addr::from(u32::from_be(v4.sin_addr.s_addr)))
                }
                libc::AF_INET6 => {
                    let v6 = &*(entry.ifa_addr as *const libc::sockaddr_in6);
                    IpAddr::V6(Ipv6Addr::from(v6.sin6_addr.s6_addr))
                }
                _ => continue,
            };
            let link_local = matches!(ip, IpAddr::V6(v6) if (v6.segments().first().copied().unwrap_or_default() & 0xffc0) == 0xfe80);
            if ip.is_loopback() || link_local {
                continue;
            }
            out.push((name, ip));
        }
        libc::freeifaddrs(head);
    }
    out.sort_by_key(|(name, ip)| (ip.is_ipv6(), name.clone()));
    out
}

/// 公网 IP 服务地址：`HELMP_PUBLIC_IP_URL` 优先（测试用），否则设置里的，再否则 ipify。
pub fn public_ip_url() -> String {
    std::env::var("HELMP_PUBLIC_IP_URL")
        .ok()
        .filter(|url| !url.trim().is_empty())
        .or_else(|| {
            let url = crate::config::get().devtools.public_ip_url.clone();
            (!url.trim().is_empty()).then_some(url)
        })
        .unwrap_or_else(|| DEFAULT_PUBLIC_IP_URL.to_string())
}

/// 演练模式下没有用 `HELMP_PUBLIC_IP_URL` 指向本地服务时不联网。
pub fn may_fetch() -> bool {
    !crate::effects::dry_run() || std::env::var_os("HELMP_PUBLIC_IP_URL").is_some()
}

/// 请求公网 IP 服务，返回的文字必须是一个 IP 地址。
pub fn public_ip(url: &str) -> Result<IpAddr> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(8)))
        .build()
        .into();
    let text = agent
        .get(url)
        .call()
        .and_then(|mut response| response.body_mut().read_to_string())
        .with_context(|| {
            trf!(
                "查询公网 IP 失败（{url}）",
                "Couldn't look up the public IP ({url})"
            )
        })?;
    text.trim().parse().with_context(|| {
        trf!(
            "{url} 返回的不是 IP 地址：{}",
            "{url} didn’t return an IP address: {}",
            text.trim().chars().take(80).collect::<String>()
        )
    })
}

pub fn family(ip: &IpAddr) -> &'static str {
    match ip {
        IpAddr::V4(_) => "IPv4",
        IpAddr::V6(_) => "IPv6",
    }
}

pub fn no_addresses() -> &'static str {
    tr("没有联网的网卡", "No connected network interface")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_addresses_skip_loopback() {
        // 127.0.0.1 对用户没用（问本机 IP 是想给别人连），列出来只会误导。
        assert!(local_addresses().iter().all(|(_, ip)| !ip.is_loopback()));
    }
}
