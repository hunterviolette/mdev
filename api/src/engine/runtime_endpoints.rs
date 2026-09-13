use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener},
    process::Command,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, RwLock,
    },
};

use anyhow::{bail, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkExposure {
    Localhost,
    Lan,
}

#[derive(Debug, Clone, Copy)]
pub struct RuntimeEndpoint {
    pub port: u16,
    pub exposure: NetworkExposure,
}

#[derive(Debug, Clone, Default)]
pub struct RuntimeEndpointManager {
    local_lan_ipv4: Arc<RwLock<Option<Ipv4Addr>>>,
    local_lan_ipv4_refreshing: Arc<AtomicBool>,
}

impl RuntimeEndpointManager {
    pub fn local_lan_ipv4(&self) -> Option<Ipv4Addr> {
        self.local_lan_ipv4
            .read()
            .ok()
            .and_then(|address| *address)
    }

    pub fn refresh_local_lan_ipv4_in_background(&self) {
        if self.local_lan_ipv4().is_some() {
            return;
        }

        if self
            .local_lan_ipv4_refreshing
            .swap(true, Ordering::AcqRel)
        {
            return;
        }

        let runtime_endpoints = self.clone();
        tokio::spawn(async move {
            let detected = tokio::task::spawn_blocking(detect_local_lan_ipv4)
                .await
                .ok()
                .flatten();

            if let Ok(mut cached) = runtime_endpoints.local_lan_ipv4.write() {
                *cached = detected;
            }

            runtime_endpoints
                .local_lan_ipv4_refreshing
                .store(false, Ordering::Release);
        });
    }

    pub fn reserve(
        &self,
        exposure: NetworkExposure,
        preferred: Option<u16>,
        range: Option<(u16, u16)>,
        excluded: &[u16],
    ) -> Result<(RuntimeEndpoint, TcpListener)> {
        if let Some(port) = preferred {
            if excluded.contains(&port) {
                bail!("preferred runtime port {} is unavailable", port);
            }
            let listener = TcpListener::bind(SocketAddr::new(bind_ip(exposure), port))
                .map_err(|_| anyhow::anyhow!("preferred runtime port {} is unavailable", port))?;
            return Ok((RuntimeEndpoint { port, exposure }, listener));
        }

        if let Some((start, end)) = range {
            for port in start..=end {
                if excluded.contains(&port) {
                    continue;
                }
                if let Ok(listener) = TcpListener::bind(SocketAddr::new(bind_ip(exposure), port)) {
                    return Ok((RuntimeEndpoint { port, exposure }, listener));
                }
            }
            bail!("no available runtime ports in range {}-{}", start, end);
        }

        let listener = TcpListener::bind(SocketAddr::new(bind_ip(exposure), 0))?;
        let port = listener.local_addr()?.port();
        Ok((RuntimeEndpoint { port, exposure }, listener))
    }

    pub fn allocate(
        &self,
        exposure: NetworkExposure,
        preferred: Option<u16>,
        range: Option<(u16, u16)>,
        excluded: &[u16],
    ) -> Result<RuntimeEndpoint> {
        let (endpoint, listener) = self.reserve(exposure, preferred, range, excluded)?;
        drop(listener);
        Ok(endpoint)
    }
}

fn detect_local_lan_ipv4() -> Option<Ipv4Addr> {
    let output = platform_ipv4_command()?;
    parse_ipv4_output(output.as_str())
}

fn parse_ipv4_output(output: &str) -> Option<Ipv4Addr> {
    output
        .split_whitespace()
        .filter_map(|value| value.trim().parse::<Ipv4Addr>().ok())
        .find(|address| valid_lan_ipv4(*address))
}

fn valid_lan_ipv4(address: Ipv4Addr) -> bool {
    let octets = address.octets();
    let private = octets[0] == 10
        || (octets[0] == 172 && (16..=31).contains(&octets[1]))
        || (octets[0] == 192 && octets[1] == 168);

    private
        && !address.is_unspecified()
        && !address.is_loopback()
        && !address.is_link_local()
        && !address.is_broadcast()
}

#[cfg(target_os = "windows")]
fn platform_ipv4_command() -> Option<String> {
    let script = r#"
$ip = Get-NetAdapter |
    Where-Object {
        $_.Status -eq 'Up' -and
        $_.HardwareInterface -eq $true
    } |
    ForEach-Object {
        $adapter = $_
        Get-NetIPAddress -InterfaceIndex $adapter.ifIndex -AddressFamily IPv4 -ErrorAction SilentlyContinue |
            Where-Object {
                $_.IPAddress -ne '127.0.0.1' -and
                $_.IPAddress -notlike '169.254.*' -and
                (
                    $_.IPAddress -like '10.*' -or
                    $_.IPAddress -like '192.168.*' -or
                    $_.IPAddress -match '^172\.(1[6-9]|2[0-9]|3[0-1])\.'
                )
            } |
            ForEach-Object {
                [PSCustomObject]@{
                    IPAddress = $_.IPAddress
                    Metric = if ($_.InterfaceMetric) { $_.InterfaceMetric } else { 999999 }
                }
            }
    } |
    Sort-Object Metric |
    Select-Object -First 1 -ExpandProperty IPAddress

if ($ip) {
    Write-Output $ip
}
"#;

    command_stdout(
        "powershell.exe",
        &[
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            script,
        ],
    )
}

#[cfg(target_os = "macos")]
fn platform_ipv4_command() -> Option<String> {
    command_stdout(
        "sh",
        &[
            "-c",
            "iface=$(route -n get default 2>/dev/null | awk '/interface:/{print $2; exit}'); if [ -n \"$iface\" ]; then ipconfig getifaddr \"$iface\" 2>/dev/null; fi",
        ],
    )
}

#[cfg(all(unix, not(target_os = "macos")))]
fn platform_ipv4_command() -> Option<String> {
    command_stdout(
        "sh",
        &[
            "-c",
            "hostname -I 2>/dev/null || ip -4 -o addr show scope global 2>/dev/null | awk '{print $4}' | cut -d/ -f1",
        ],
    )
}

fn command_stdout(program: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(program).args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8(output.stdout).ok()?;
    let stdout = stdout.trim();
    if stdout.is_empty() {
        None
    } else {
        Some(stdout.to_string())
    }
}

fn bind_ip(exposure: NetworkExposure) -> IpAddr {
    match exposure {
        NetworkExposure::Localhost => IpAddr::V4(Ipv4Addr::LOCALHOST),
        NetworkExposure::Lan => IpAddr::V4(Ipv4Addr::UNSPECIFIED),
    }
}

fn port_available(exposure: NetworkExposure, port: u16) -> bool {
    TcpListener::bind(SocketAddr::new(bind_ip(exposure), port)).is_ok()
}
