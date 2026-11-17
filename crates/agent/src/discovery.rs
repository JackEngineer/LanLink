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
const MAX_OBSERVATIONS: usize = 256;
fn service(device: &Device, port: u16, incarnation: &str) -> Result<ServiceInfo> {
    let mut end = device.name.len().min(250);
    while !device.name.is_char_boundary(end) {
        end -= 1;
    }
    let properties = [
        ("id", device.id.as_str()),
        ("name", &device.name[..end]),
        ("os", device.platform.as_str()),
        ("version", VERSION),
    ];
    Ok(ServiceInfo::new(
        SERVICE,
        &format!("{}-{incarnation}", &device.id[..16]),
        &format!("lanlink-{}-{incarnation}.local.", &device.id[..16]),
        "",
        port,
        &properties[..],
    )?
    .enable_addr_auto())
}
fn record_observation(
    names: &mut HashMap<String, String>,
    observations: &mut HashMap<String, Observation>,
    fullname: &str,
    observation: Observation,
) -> bool {
    let id = &observation.peer.device.id;
    if (observations.len() >= MAX_OBSERVATIONS && !observations.contains_key(id))
        || (names.len() >= MAX_OBSERVATIONS && !names.values().any(|existing| existing == id))
        || names.get(fullname).is_some_and(|existing| existing != id)
    {
        return false;
    }
    names.retain(|_, existing| existing != id);
    names.insert(fullname.to_owned(), id.clone());
    observations.insert(id.clone(), observation);
    true
}

fn remove_observation(
    names: &mut HashMap<String, String>,
    observations: &mut HashMap<String, Observation>,
    fullname: &str,
) -> bool {
    if let Some(id) = names.remove(fullname) {
        observations.remove(&id);
        return true;
    }
    false
}
pub async fn start(state: Arc<State>) -> Result<ServiceDaemon> {
    let daemon = ServiceDaemon::new()?;
    let incarnation = uuid::Uuid::new_v4().simple().to_string();
    daemon.register(service(
        &*state.device.read().await,
        state.tcp_port,
        &incarnation,
    )?)?;
    let receiver = daemon.browse(SERVICE)?;
    let update_daemon = daemon.clone();
    tokio::spawn(async move {
        let mut names: HashMap<String, String> = HashMap::new();
        let mut tick = interval(Duration::from_secs(20));
        let mut advertised = state.device.read().await.name.clone();
        loop {
            tokio::select! {
                _ = tick.tick() => {
                    let device = state.device.read().await.clone();
                    if device.name != advertised && let Ok(info) = service(&device, state.tcp_port, &incarnation) && update_daemon.register(info).is_ok() { advertised = device.name; }
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
                            let mut observations = state.observations.write().await;
                            if !record_observation(&mut names, &mut observations, info.get_fullname(), Observation { peer: Peer { device: Device { id: id.into(), name, platform }, addresses, port: info.get_port(), online: true, paired: false }, seen: now_ms() }) { continue; }
                            drop(observations); state.changed();
                        }
                        Ok(ServiceEvent::ServiceRemoved(_, name)) => {
                            if remove_observation(&mut names, &mut *state.observations.write().await, &name) { state.changed(); }
                        }
                        Err(_) => break,
                        _ => {},
                    }
                }
            }
        }
    });
    Ok(daemon)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observation(id: usize) -> Observation {
        Observation {
            peer: Peer {
                device: Device {
                    id: format!("{id:064x}"),
                    name: "test".into(),
                    platform: "test".into(),
                },
                addresses: vec!["192.168.1.2".into()],
                port: 38762,
                online: true,
                paired: false,
            },
            seen: now_ms(),
        }
    }

    #[test]
    fn removed_devices_release_discovery_capacity_and_names_stay_bounded() {
        let mut names = HashMap::new();
        let mut observations = HashMap::new();
        for id in 0..MAX_OBSERVATIONS {
            assert!(record_observation(
                &mut names,
                &mut observations,
                &format!("device-{id}"),
                observation(id)
            ));
        }
        for id in MAX_OBSERVATIONS..MAX_OBSERVATIONS * 2 {
            assert!(!record_observation(
                &mut names,
                &mut observations,
                &format!("device-{id}"),
                observation(id)
            ));
        }
        assert_eq!(names.len(), MAX_OBSERVATIONS);
        assert_eq!(observations.len(), MAX_OBSERVATIONS);
        assert!(remove_observation(
            &mut names,
            &mut observations,
            "device-0"
        ));
        assert!(record_observation(
            &mut names,
            &mut observations,
            "new-device",
            observation(999)
        ));
        assert!(!observations.contains_key(&format!("{:064x}", 0)));
        for alias in 0..1000 {
            assert!(record_observation(
                &mut names,
                &mut observations,
                &format!("alias-{alias}"),
                observation(999)
            ));
        }
        assert_eq!(names.len(), MAX_OBSERVATIONS);
        assert!(!remove_observation(
            &mut names,
            &mut observations,
            "new-device"
        ));
        assert!(observations.contains_key(&format!("{:064x}", 999)));
    }

    #[test]
    fn unicode_device_name_fits_mdns_without_changing_full_name() -> Result<()> {
        let name = "🌊".repeat(80);
        let device = Device {
            id: "a".repeat(64),
            name: name.clone(),
            platform: "macos".into(),
        };
        let info = service(&device, 38762, &"b".repeat(32))?;
        let advertised = info.get_property_val_str("name").unwrap();
        assert!(advertised.len() <= 250);
        assert_eq!(advertised, "🌊".repeat(62));
        assert_eq!(device.name, name);
        let restarted = service(&device, 38762, &"c".repeat(32))?;
        assert_ne!(info.get_fullname(), restarted.get_fullname());
        assert_ne!(info.get_hostname(), restarted.get_hostname());
        assert_eq!(
            info.get_property_val_str("id"),
            restarted.get_property_val_str("id")
        );
        Ok(())
    }
}
