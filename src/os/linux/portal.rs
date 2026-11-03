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
    request_on(Bus::session()?, enable, command, reason, ANSWER_TIMEOUT)
}

fn request_on(
    mut bus: Bus,
    enable: bool,
    command: &str,
    reason: &str,
    timeout: Duration,
) -> Option<bool> {
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
    let deadline = Instant::now() + timeout;
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

#[cfg(test)]
mod tests {
    use super::super::dbus::{METHOD_CALL, TestBus};
    use super::*;

    fn own(bus: &mut Bus, name: &str) {
        let m = Message::call(
            "org.freedesktop.DBus",
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
            "RequestName",
            vec![Value::Str(name.into()), Value::U32(4)],
        );
        assert!(bus.call(&m).is_some());
    }

    /// A stand-in portal: answers one RequestBackground with `code`,
    /// granting what was asked; a stranger first sends a forged answer
    /// straight to the app, which must be ignored.
    fn portal(addr: String, code: u32) -> std::thread::JoinHandle<Vec<Value>> {
        let mut p = Bus::connect(&addr).expect("portal connects");
        own(&mut p, PORTAL);
        std::thread::spawn(move || {
            for _ in 0..300 {
                for m in p.poll() {
                    if m.kind != METHOD_CALL || m.member.as_deref() != Some("RequestBackground") {
                        continue;
                    }
                    let Some(Value::Array(_, opts)) = m.body.get(1) else {
                        panic!("no options")
                    };
                    let opt = |k: &str| {
                        opts.iter().find_map(|e| match e {
                            Value::Dict(n, v) if n.as_str() == Some(k) => match v.as_ref() {
                                Value::Variant(x) => Some((**x).clone()),
                                _ => None,
                            },
                            _ => None,
                        })
                    };
                    let token = opt("handle_token").and_then(|v| v.as_str().map(String::from));
                    let caller = m.sender.clone().unwrap();
                    let path = format!(
                        "{PORTAL_PATH}/request/{}/{}",
                        caller.trim_start_matches(':').replace('.', "_"),
                        token.unwrap()
                    );
                    p.send(&Message::reply_to(&m, vec![Value::Path(path.clone())]));
                    let answer = |c: u32, on: bool| {
                        Message::signal(
                            &path,
                            "org.freedesktop.portal.Request",
                            "Response",
                            vec![
                                Value::U32(c),
                                Value::Array(
                                    "{sv}".into(),
                                    vec![
                                        Value::entry("background", Value::Bool(true)),
                                        Value::entry("autostart", Value::Bool(on)),
                                    ],
                                ),
                            ],
                        )
                    };
                    // The forgery: "on", sent directly, not by the portal.
                    let mut stranger = Bus::connect(&addr).unwrap();
                    let mut forged = answer(0, true);
                    forged.destination = Some(caller);
                    stranger.send(&forged);
                    std::thread::sleep(Duration::from_millis(100));
                    let asked = opt("autostart") == Some(Value::Bool(true));
                    p.send(&answer(code, asked && code == 0));
                    return vec![
                        opt("autostart").unwrap(),
                        opt("commandline").unwrap(),
                        opt("reason").unwrap(),
                    ];
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            panic!("no request")
        })
    }

    #[test]
    fn background_portal() {
        let Some(tb) = TestBus::start("portal", "") else {
            return; // no dbus-daemon here
        };
        let t = Duration::from_secs(10);
        // Granted: login start is on; the request carried the command.
        let th = portal(tb.addr.clone(), 0);
        let on = request_on(
            Bus::connect(&tb.addr).unwrap(),
            true,
            "weeknumber",
            "why",
            t,
        );
        let asked = th.join().unwrap();
        assert_eq!(on, Some(true));
        assert_eq!(asked[0], Value::Bool(true));
        assert_eq!(
            asked[1],
            Value::Array("s".into(), vec![Value::Str("weeknumber".into())])
        );
        assert_eq!(asked[2], Value::Str("why".into()));
        // Turned off: off.
        let th = portal(tb.addr.clone(), 0);
        let on = request_on(Bus::connect(&tb.addr).unwrap(), false, "weeknumber", "", t);
        th.join().unwrap();
        assert_eq!(on, Some(false));
        // Refused or cancelled by the user (forged "on" ignored): off.
        let th = portal(tb.addr.clone(), 1);
        let on = request_on(Bus::connect(&tb.addr).unwrap(), true, "weeknumber", "", t);
        th.join().unwrap();
        assert_eq!(on, Some(false));
        // No portal on the bus: no answer.
        let on = request_on(
            Bus::connect(&tb.addr).unwrap(),
            true,
            "weeknumber",
            "",
            Duration::from_millis(300),
        );
        assert_eq!(on, None);
    }
}
