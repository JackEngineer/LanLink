use crate::{
    network::local_ip,
    state::{Observation, State},
};
use anyhow::Result;
use lanlink_protocol::*;
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use std::{collections::HashMap, sync::Arc};
use tokio::time::{Duration, interval};
const SERVICE: &str = "_lanlink._tcp.local.";
fn service(device: &Device, port: u16) -> Result<ServiceInfo> {
    let properties = [
        ("id", device.id.as_str()),
        ("name", device.name.as_str()),
        ("os", device.platform.as_str()),
        ("version", VERSION),
    ];
    Ok(ServiceInfo::new(
        SERVICE,
        &device.id[..32],
        &format!("lanlink-{}.local.", &device.id[..16]),
        "",
        port,
        &properties[..],
    )?
    .enable_addr_auto())
}
pub async fn start(state: Arc<State>) -> Result<ServiceDaemon> {
    let daemon = ServiceDaemon::new()?;
    daemon.register(service(&*state.device.read().await, state.tcp_port)?)?;
    let receiver = daemon.browse(SERVICE)?;
    let update_daemon = daemon.clone();
    tokio::spawn(async move {
        let mut names: HashMap<String, String> = HashMap::new();
        let mut tick = interval(Duration::from_secs(20));
        let mut advertised = state.device.read().await.name.clone();
        loop {
            tokio::select! {
                _ = tick.tick() => {
                    for fullname in names.keys() { let _ = update_daemon.verify(fullname.clone(), Duration::from_secs(10)); }
                    let device = state.device.read().await.clone();
                    if device.name != advertised && let Ok(info) = service(&device, state.tcp_port) && update_daemon.register(info).is_ok() { advertised = device.name; }
                }
                event = receiver.recv_async() => {
                    match event {
                        Ok(ServiceEvent::ServiceResolved(info)) => {
                            let Some(id) = info.get_property_val_str("id") else { continue; };
                            if !valid_id(id) || id == state.device.read().await.id { continue; }
                            let addresses: Vec<_> = info.get_addresses_v4().into_iter().filter(|ip| local_ip(std::net::IpAddr::V4(*ip))).map(|ip| ip.to_string()).collect();
                            if addresses.is_empty() || info.get_port() == 0 { continue; }
                            let name = info.get_property_val_str("name").unwrap_or("LANLink device").chars().take(80).collect();
                            let platform = info.get_property_val_str("os").unwrap_or("unknown").chars().take(32).collect();
                            names.insert(info.get_fullname().to_owned(), id.to_owned());
                            let mut observations = state.observations.write().await;
                            if observations.len() >= 256 && !observations.contains_key(id) { continue; }
                            observations.insert(id.into(), Observation { peer: Peer { device: Device { id: id.into(), name, platform }, addresses, port: info.get_port(), online: true, paired: false }, seen: now_ms() });
                            drop(observations); state.changed();
                        }
                        Ok(ServiceEvent::ServiceRemoved(_, name)) => { if let Some(id) = names.remove(&name) { if let Some(obs) = state.observations.write().await.get_mut(&id) { obs.seen = 0; obs.peer.online = false; } state.changed(); } }
                        Err(_) => break,
                        _ => {},
                    }
                }
            }
        }
    });
    Ok(daemon)
}
