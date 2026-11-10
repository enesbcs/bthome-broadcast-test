//! BlueZ `org.bluez.LEAdvertisement1` backend.
//!
//! BlueZ owns the actual HCI advertising; the process exports a D-Bus object
//! that BlueZ reads and registers. The advertised data is the BTHome v2
//! service data, plus the hostname-derived local name.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use zbus::fdo::ManagedObjects;
use zbus::{block_on, interface, Connection, ObjectPath, OwnedObjectPath, OwnedVariant, Variant};

use super::{BleError, Broadcaster};
use crate::bthome;

/// Object path exported to BlueZ.
pub const ADVERTISEMENT_PATH: &str = "/org/bletemp/advertisement1";

const BLUEZ: &str = "org.bluez";
const ADAPTER_INTERFACE: &str = "org.bluez.Adapter1";
const ADVERTISING_MANAGER_INTERFACE: &str = "org.bluez.LEAdvertisingManager1";
const PROPERTIES_INTERFACE: &str = "org.freedesktop.DBus.Properties";
const OBJECT_MANAGER_INTERFACE: &str = "org.freedesktop.DBus.ObjectManager";

/// Current advertisement data shared with the D-Bus object.
#[derive(Clone, Debug, PartialEq, Eq)]
struct AdState {
    name: String,
    payload: [u8; 4],
}

impl Default for AdState {
    fn default() -> Self {
        Self {
            name: String::new(),
            payload: [bthome::DEVICE_INFO, bthome::OBJECT_TEMPERATURE, 0, 0],
        }
    }
}

impl AdState {
    fn new(name: &str, celsius: f64) -> Option<Self> {
        let payload = bthome::service_data(celsius)?;
        Some(Self {
            name: name.to_string(),
            payload,
        })
    }

    /// `ServiceData` property: UUID -> BTHome payload bytes.
    fn service_data(&self) -> HashMap<String, OwnedVariant> {
        let variant = OwnedVariant::try_from(Variant::from(self.payload.to_vec()))
            .expect("byte array is always a valid variant");
        HashMap::from([("FCD2".to_string(), variant)])
    }
}

/// D-Bus interface consumed by BlueZ.
#[derive(Clone)]
struct Advertisement {
    state: Arc<Mutex<AdState>>,
}

impl Advertisement {
    fn state(&self) -> MutexGuard<'_, AdState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[interface(name = "org.bluez.LEAdvertisement1")]
impl Advertisement {
    #[zbus(property)]
    fn r#type(&self) -> String {
        "peripheral".to_string()
    }

    #[zbus(property)]
    fn local_name(&self) -> String {
        self.state().name.clone()
    }

    #[zbus(property)]
    fn service_data(&self) -> HashMap<String, OwnedVariant> {
        self.state().service_data()
    }

    #[zbus(property)]
    fn discoverable(&self) -> bool {
        true
    }
}

/// Adapter data needed by the broadcaster and by `--diagnose`.
#[derive(Clone, Debug)]
struct Adapter {
    path: OwnedObjectPath,
    address: String,
    powered: bool,
}

/// BlueZ-backed advertiser.
pub struct BluezBroadcaster {
    connection: Connection,
    adapter: OwnedObjectPath,
    state: Arc<Mutex<AdState>>,
    registered: bool,
}

impl BluezBroadcaster {
    /// Connects to BlueZ and exports the advertisement object. Advertising
    /// starts with the first [`Broadcaster::update`] call.
    pub fn connect() -> Result<Self, BleError> {
        let connection = block_on(Connection::system()).map_err(|error| {
            BleError::Unavailable(format!("cannot connect to the system D-Bus: {error}"))
        })?;

        let adapter = find_adapter(&connection)?;
        ensure_powered(&connection, &adapter)?;

        let state = Arc::new(Mutex::new(AdState::default()));
        let advertisement = Advertisement {
            state: Arc::clone(&state),
        };
        match block_on(
            connection
                .object_server()
                .at(ADVERTISEMENT_PATH, advertisement),
        ) {
            Ok(None) => {}
            Ok(Some(_)) => {
                return Err(BleError::Failed(format!(
                    "D-Bus object {ADVERTISEMENT_PATH} is already in use"
                )));
            }
            Err(error) => {
                return Err(BleError::Failed(format!(
                    "cannot export the advertisement object: {error}"
                )));
            }
        }

        Ok(Self {
            connection,
            adapter: adapter.path,
            state,
            registered: false,
        })
    }

    fn register(&self) -> Result<(), BleError> {
        let path = advertisement_path();
        let options: HashMap<String, OwnedVariant> = HashMap::new();
        block_on(self.connection.call_method(
            Some(BLUEZ),
            self.adapter.as_str(),
            Some(ADVERTISING_MANAGER_INTERFACE),
            "RegisterAdvertisement",
            &(path, options),
        ))
        .map_err(|error| BleError::Failed(format!("RegisterAdvertisement failed: {error}")))?;
        Ok(())
    }

    fn unregister(&self) -> Result<(), BleError> {
        let path = advertisement_path();
        block_on(self.connection.call_method(
            Some(BLUEZ),
            self.adapter.as_str(),
            Some(ADVERTISING_MANAGER_INTERFACE),
            "UnregisterAdvertisement",
            &(path,),
        ))
        .map_err(|error| BleError::Failed(format!("UnregisterAdvertisement failed: {error}")))?;
        Ok(())
    }
}

impl Broadcaster for BluezBroadcaster {
    fn update(&mut self, name: &str, celsius: f64) -> Result<(), BleError> {
        let next = AdState::new(name, celsius)
            .ok_or_else(|| BleError::Failed(format!("{celsius} °C cannot be encoded as BTHome")))?;

        {
            let mut current = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if self.registered && *current == next {
                return Ok(());
            }
            *current = next;
        }

        if self.registered {
            self.unregister()?;
            self.registered = false;
        }
        self.register()?;
        self.registered = true;
        Ok(())
    }

    fn suspend(&mut self) -> Result<(), BleError> {
        if self.registered {
            self.unregister()?;
            self.registered = false;
        }
        Ok(())
    }

    fn shutdown(&mut self) -> Result<(), BleError> {
        self.suspend()
    }
}

/// Status printed by `--diagnose`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BleInfo {
    pub adapter: String,
    pub address: String,
    pub powered: bool,
    pub advertising: bool,
}

/// Probes BlueZ without starting an advertisement.
pub fn probe() -> Result<BleInfo, BleError> {
    let connection = block_on(Connection::system()).map_err(|error| {
        BleError::Unavailable(format!("cannot connect to the system D-Bus: {error}"))
    })?;
    let adapter = find_adapter(&connection)?;
    Ok(BleInfo {
        adapter: adapter.path.as_str().to_string(),
        address: adapter.address,
        powered: adapter.powered,
        advertising: true,
    })
}

fn advertisement_path() -> ObjectPath<'static> {
    ObjectPath::try_from(ADVERTISEMENT_PATH).expect("the advertisement path is valid")
}

fn find_adapter(connection: &Connection) -> Result<Adapter, BleError> {
    let reply = block_on(connection.call_method(
        Some(BLUEZ),
        "/",
        Some(OBJECT_MANAGER_INTERFACE),
        "GetManagedObjects",
        &(),
    ))
    .map_err(|error| {
        BleError::Unavailable(format!("BlueZ is not available on the system bus: {error}"))
    })?;
    let body = reply.body();
    let objects: ManagedObjects = body.deserialize().map_err(|error| {
        BleError::Failed(format!("cannot parse the BlueZ object tree: {error}"))
    })?;

    let mut adapters = Vec::new();
    for (path, interfaces) in objects {
        let Some(properties) = interfaces
            .iter()
            .find(|(name, _)| name.as_str() == ADAPTER_INTERFACE)
            .map(|(_, properties)| properties)
        else {
            continue;
        };
        let has_manager = interfaces
            .keys()
            .any(|name| name.as_str() == ADVERTISING_MANAGER_INTERFACE);
        if !has_manager {
            continue;
        }

        let address = properties
            .get("Address")
            .and_then(|value| value.try_clone().ok())
            .and_then(|value| String::try_from(value).ok())
            .unwrap_or_else(|| "unknown".to_string());
        let powered = properties
            .get("Powered")
            .and_then(|value| value.try_clone().ok())
            .and_then(|value| bool::try_from(value).ok())
            .unwrap_or(false);

        adapters.push(Adapter {
            path,
            address,
            powered,
        });
    }

    // Prefer an adapter that is already powered on, then use a stable order so
    // that multiple adapters do not make the choice random.
    adapters.sort_by(|left, right| left.path.as_str().cmp(right.path.as_str()));
    if let Some(index) = adapters.iter().position(|adapter| adapter.powered) {
        return Ok(adapters.swap_remove(index));
    }
    adapters.into_iter().next().ok_or_else(|| {
        BleError::Unavailable("no Bluetooth adapter with LE advertising support".to_string())
    })
}

fn ensure_powered(connection: &Connection, adapter: &Adapter) -> Result<(), BleError> {
    if adapter.powered {
        return Ok(());
    }

    block_on(connection.call_method(
        Some(BLUEZ),
        adapter.path.as_str(),
        Some(PROPERTIES_INTERFACE),
        "Set",
        &(ADAPTER_INTERFACE, "Powered", Variant::from(true)),
    ))
    .map_err(|error| BleError::Failed(format!("cannot power on the Bluetooth adapter: {error}")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn advertisement_state_uses_bthome_payload() {
        let state = AdState::new("host-C", 48.5).unwrap();
        assert_eq!(state.payload, [0x40, 0x02, 0xF2, 0x12]);
    }

    #[test]
    fn service_data_map_contains_the_uuid() {
        let state = AdState::new("host-C", -12.34).unwrap();
        let map = state.service_data();
        let value = map.get("FCD2").expect("service data entry");
        let bytes = Vec::<u8>::try_from(value.try_clone().unwrap()).unwrap();
        assert_eq!(bytes, vec![0x40, 0x02, 0x2E, 0xFB]);
    }

    #[test]
    fn state_comparison_detects_name_and_value_changes() {
        let base = AdState::new("host-C", 42.0).unwrap();
        assert_eq!(base, AdState::new("host-C", 42.0).unwrap());
        assert_ne!(base, AdState::new("host-G", 42.0).unwrap());
        assert_ne!(base, AdState::new("host-C", 42.01).unwrap());
    }

    #[test]
    fn unencodable_temperature_is_rejected() {
        assert!(AdState::new("host-C", f64::NAN).is_none());
        assert!(AdState::new("host-C", 400.0).is_none());
    }

    #[test]
    fn interface_properties_reflect_the_state() {
        let advertisement = Advertisement {
            state: Arc::new(Mutex::new(AdState::new("host-G", 61.5).unwrap())),
        };
        assert_eq!(advertisement.r#type(), "peripheral");
        assert_eq!(advertisement.local_name(), "host-G");
        assert!(advertisement.discoverable());
        assert!(advertisement.service_data().contains_key("FCD2"));
    }
}
