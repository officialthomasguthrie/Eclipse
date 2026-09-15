//! The system bus from a client's side. [`reason`] turns an error reply into the sentence a person
//! reads. With the `bus` feature this module also opens the connection and the proxies that
//! [`crate::quasar`] and [`crate::orbit`] ask through.

#[cfg(feature = "bus")]
use std::time::Duration;

#[cfg(feature = "bus")]
use zbus::DBusError as _;

use crate::Component;

/// How long reading a property may take. The services answer those from memory.
#[cfg(feature = "bus")]
pub(crate) const PROPERTY_TIMEOUT: Duration = Duration::from_secs(10);

/// The sentence for an error reply from `component`, from the D-Bus error name and the message
/// that came with it.
#[must_use]
pub fn reason(component: Component, error: &str, detail: Option<&str>) -> String {
    let name = component.display_name();
    match error {
        "org.freedesktop.DBus.Error.ServiceUnknown"
        | "org.freedesktop.DBus.Error.NameHasNoOwner" => format!("{name} is not running."),
        "org.freedesktop.DBus.Error.NoReply" | "org.freedesktop.DBus.Error.Timeout" => {
            format!("{name} took too long to answer.")
        }
        _ => detail
            .map(str::trim)
            .filter(|detail| !detail.is_empty())
            .map_or_else(|| format!("{name} could not answer."), ToString::to_string),
    }
}

/// A connection to the system bus whose method calls give up after `timeout`.
#[cfg(feature = "bus")]
pub(crate) fn connect(timeout: Duration) -> Result<zbus::blocking::Connection, String> {
    zbus::blocking::connection::Builder::system()
        .map(|builder| builder.method_timeout(timeout))
        .and_then(zbus::blocking::connection::Builder::build)
        .map_err(|e| format!("Could not reach the system bus: {e}"))
}

/// A proxy for the component's own interface. Properties are read fresh every time, never from
/// a cache that could be out of date.
#[cfg(feature = "bus")]
pub(crate) fn proxy(
    connection: &zbus::blocking::Connection,
    component: Component,
) -> Result<zbus::blocking::Proxy<'static>, String> {
    build(connection, component).map_err(|e| sentence(component, e))
}

#[cfg(feature = "bus")]
fn build(
    connection: &zbus::blocking::Connection,
    component: Component,
) -> zbus::Result<zbus::blocking::Proxy<'static>> {
    let name = component.dbus_name();
    zbus::blocking::proxy::Builder::new(connection)
        .destination(name.clone())?
        .path(component.dbus_path())?
        .interface(name)?
        .cache_properties(zbus::proxy::CacheProperties::No)
        .build()
}

/// The sentence for anything that went wrong while talking to `component`.
#[cfg(feature = "bus")]
pub(crate) fn sentence(component: Component, error: zbus::Error) -> String {
    match error {
        zbus::Error::MethodError(name, detail, _) => {
            reason(component, name.as_str(), detail.as_deref())
        }
        zbus::Error::FDO(error) => reason(component, error.name().as_str(), error.description()),
        zbus::Error::InputOutput(error) if error.kind() == std::io::ErrorKind::TimedOut => {
            format!("{} took too long to answer.", component.display_name())
        }
        other => format!("Could not talk to {}: {other}", component.display_name()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_on_the_bus_read_as_sentences() {
        assert_eq!(
            reason(
                Component::Quasar,
                "org.freedesktop.DBus.Error.ServiceUnknown",
                None
            ),
            "Quasar is not running."
        );
        assert_eq!(
            reason(
                Component::Orbit,
                "org.freedesktop.DBus.Error.NameHasNoOwner",
                Some("The name is not activatable")
            ),
            "Orbit is not running."
        );
        assert_eq!(
            reason(
                Component::Quasar,
                "org.freedesktop.DBus.Error.NoReply",
                Some("Did not receive a reply")
            ),
            "Quasar took too long to answer."
        );
        assert_eq!(
            reason(
                Component::Quasar,
                "org.freedesktop.DBus.Error.Failed",
                Some("The model is still loading. Try again in a moment.")
            ),
            "The model is still loading. Try again in a moment."
        );
        assert_eq!(
            reason(
                Component::Quasar,
                "org.freedesktop.DBus.Error.Failed",
                Some(" ")
            ),
            "Quasar could not answer."
        );
    }
}
