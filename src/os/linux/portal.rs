//! The Background portal (org.freedesktop.portal.Background), through which
//! the Flatpak edition starts at login: the sandbox cannot write the
//! desktop's autostart folder, so the portal (outside it) writes the entry
//! for this app and asks the user first where the desktop wants that. One
//! request on its own session-bus connection; the portal answers with a
//! Response signal on the request's object.

use super::dbus::{Bus, Message, SIGNAL, Value};
use std::time::{Duration, Instant};

const PORTAL: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
/// The portal may show the user a question first; wait this long for it.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(120);

fn add_match(bus: &mut Bus, path: &str) -> Option<()> {
    let rule = format!(
        "type='signal',sender='{PORTAL}',interface='org.freedesktop.portal.Request',\
         member='Response',path='{path}'"
    );
    bus.call(&Message::call(
        "org.freedesktop.DBus",
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
        "AddMatch",
        vec![Value::Str(rule)],
    ))
    .map(|_| ())
}

/// Ask the portal to start `command` at login (`enable`) or no longer.
/// Some(whether login start is now on) once the portal has answered; None
/// when there is no portal or no answer.
pub fn request_autostart(enable: bool, command: &str, reason: &str) -> Option<bool> {
    let mut bus = Bus::session()?;
    let token = format!("weeknumber{}", std::process::id());
    let sender = bus.unique_name.trim_start_matches(':').replace('.', "_");
    let mut handle = format!("{PORTAL_PATH}/request/{sender}/{token}");
    // Subscribe before asking, so a quick answer is not missed.
    add_match(&mut bus, &handle)?;
    let options = vec![
        Value::entry("handle_token", Value::Str(token)),
        Value::entry("reason", Value::Str(reason.into())),
        Value::entry("autostart", Value::Bool(enable)),
        Value::entry(
            "commandline",
            Value::Array("s".into(), vec![Value::Str(command.into())]),
        ),
        Value::entry("dbus-activatable", Value::Bool(false)),
    ];
    let reply = bus.call(&Message::call(
        PORTAL,
        PORTAL_PATH,
        "org.freedesktop.portal.Background",
        "RequestBackground",
        vec![
            Value::Str(String::new()),
            Value::Array("{sv}".into(), options),
        ],
    ))?;
    // Portals older than version 0.9 pick their own request path.
    if let Some(path) = reply.body.first().and_then(Value::as_str)
        && path != handle
    {
        handle = path.to_string();
        add_match(&mut bus, &handle)?;
    }
    // Only the portal itself may answer.
    let owner = bus
        .call(&Message::call(
            "org.freedesktop.DBus",
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
            "GetNameOwner",
            vec![Value::Str(PORTAL.into())],
        ))?
        .body
        .first()
        .and_then(Value::as_str)?
        .to_string();
    let deadline = Instant::now() + ANSWER_TIMEOUT;
    while Instant::now() < deadline {
        for m in bus.poll() {
            if m.kind != SIGNAL
                || m.member.as_deref() != Some("Response")
                || m.path.as_deref() != Some(handle.as_str())
                || m.sender.as_deref() != Some(owner.as_str())
            {
                continue;
            }
            return Some(match (m.body.first(), m.body.get(1)) {
                // 0: answered; the results say what is in force now.
                (Some(Value::U32(0)), Some(Value::Array(_, results))) => {
                    results.iter().any(|e| match e {
                        Value::Dict(k, v) => {
                            k.as_str() == Some("autostart")
                                && matches!(v.as_ref(), Value::Variant(b) if **b == Value::Bool(true))
                        }
                        _ => false,
                    })
                }
                // Cancelled or refused: nothing was turned on.
                _ => false,
            });
        }
        super::super::unix::sys::wait_readable(&[bus.fd()], 500);
    }
    None
}
