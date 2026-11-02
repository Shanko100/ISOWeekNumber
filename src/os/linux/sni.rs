//! The tray icon over D-Bus (StatusNotifierItem), as KDE Plasma, GNOME with
//! the AppIndicator extension, Cinnamon, Budgie, LXQt, Xfce and the
//! wlroots panels show them. The icon is sent as pixels; the menu is
//! published with the com.canonical.dbusmenu interface and drawn by the
//! panel itself; clicks come back as method calls.

use super::dbus::{Bus, Message, Value};

const ITEM_PATH: &str = "/StatusNotifierItem";
const MENU_PATH: &str = "/MenuBar";
const ITEM_IFACE: &str = "org.kde.StatusNotifierItem";
const MENU_IFACE: &str = "com.canonical.dbusmenu";
const PROPS_IFACE: &str = "org.freedesktop.DBus.Properties";
const WATCHER: &str = "org.kde.StatusNotifierWatcher";
const SEPARATOR_BASE: i32 = 1000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MenuItem {
    Item {
        id: u32,
        label: String,
        checked: Option<bool>,
    },
    Separator,
}

#[derive(Debug, PartialEq, Eq)]
pub enum TrayEvent {
    /// A click on the icon, at this screen position (0, 0 when the panel
    /// does not say).
    Activate(i32, i32),
    ContextMenu,
    Menu(u32),
    /// The panel asks for the menu: refresh its check marks first.
    AboutToShow,
}

pub struct Sni {
    bus: Bus,
    name: String,
    registered: bool,
    /// (width, height, ARGB32 big-endian pixels)
    icons: Vec<(i32, i32, Vec<u8>)>,
    title: String,
    tooltip: String,
    menu: Vec<MenuItem>,
    revision: u32,
}

const INTROSPECT_ITEM: &str = r#"<!DOCTYPE node PUBLIC "-//freedesktop//DTD D-BUS Object Introspection 1.0//EN" "http://www.freedesktop.org/standards/dbus/1.0/introspect.dtd">
<node>
 <interface name="org.kde.StatusNotifierItem">
  <property name="Category" type="s" access="read"/>
  <property name="Id" type="s" access="read"/>
  <property name="Title" type="s" access="read"/>
  <property name="Status" type="s" access="read"/>
  <property name="WindowId" type="i" access="read"/>
  <property name="IconName" type="s" access="read"/>
  <property name="IconPixmap" type="a(iiay)" access="read"/>
  <property name="ToolTip" type="(sa(iiay)ss)" access="read"/>
  <property name="ItemIsMenu" type="b" access="read"/>
  <property name="Menu" type="o" access="read"/>
  <method name="Activate"><arg name="x" type="i" direction="in"/><arg name="y" type="i" direction="in"/></method>
  <method name="SecondaryActivate"><arg name="x" type="i" direction="in"/><arg name="y" type="i" direction="in"/></method>
  <method name="ContextMenu"><arg name="x" type="i" direction="in"/><arg name="y" type="i" direction="in"/></method>
  <method name="Scroll"><arg name="delta" type="i" direction="in"/><arg name="orientation" type="s" direction="in"/></method>
  <signal name="NewIcon"/>
  <signal name="NewToolTip"/>
  <signal name="NewTitle"/>
  <signal name="NewStatus"><arg name="status" type="s"/></signal>
 </interface>
 <interface name="org.freedesktop.DBus.Properties">
  <method name="Get"><arg type="s" direction="in"/><arg type="s" direction="in"/><arg type="v" direction="out"/></method>
  <method name="GetAll"><arg type="s" direction="in"/><arg type="a{sv}" direction="out"/></method>
 </interface>
</node>"#;

const INTROSPECT_MENU: &str = r#"<!DOCTYPE node PUBLIC "-//freedesktop//DTD D-BUS Object Introspection 1.0//EN" "http://www.freedesktop.org/standards/dbus/1.0/introspect.dtd">
<node>
 <interface name="com.canonical.dbusmenu">
  <property name="Version" type="u" access="read"/>
  <property name="TextDirection" type="s" access="read"/>
  <property name="Status" type="s" access="read"/>
  <property name="IconThemePath" type="as" access="read"/>
  <method name="GetLayout"><arg type="i" direction="in"/><arg type="i" direction="in"/><arg type="as" direction="in"/><arg type="u" direction="out"/><arg type="(ia{sv}av)" direction="out"/></method>
  <method name="GetGroupProperties"><arg type="ai" direction="in"/><arg type="as" direction="in"/><arg type="a(ia{sv})" direction="out"/></method>
  <method name="GetProperty"><arg type="i" direction="in"/><arg type="s" direction="in"/><arg type="v" direction="out"/></method>
  <method name="Event"><arg type="i" direction="in"/><arg type="s" direction="in"/><arg type="v" direction="in"/><arg type="u" direction="in"/></method>
  <method name="EventGroup"><arg type="a(isvu)" direction="in"/><arg type="ai" direction="out"/></method>
  <method name="AboutToShow"><arg type="i" direction="in"/><arg type="b" direction="out"/></method>
  <method name="AboutToShowGroup"><arg type="ai" direction="in"/><arg type="ai" direction="out"/><arg type="ai" direction="out"/></method>
  <signal name="LayoutUpdated"><arg type="u"/><arg type="i"/></signal>
  <signal name="ItemsPropertiesUpdated"><arg type="a(ia{sv})"/><arg type="a(ias)"/></signal>
 </interface>
</node>"#;

impl Sni {
    /// Connect to the session bus and offer the item; `registered()` says
    /// whether a panel has taken it.
    pub fn start() -> Option<Sni> {
        Sni::start_on(Bus::session()?)
    }

    fn start_on(mut bus: Bus) -> Option<Sni> {
        let name = format!("org.kde.StatusNotifierItem-{}-1", std::process::id());
        let req = Message::call(
            "org.freedesktop.DBus",
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
            "RequestName",
            vec![Value::Str(name.clone()), Value::U32(4)],
        );
        bus.call(&req)?;
        let rule = format!(
            "type='signal',sender='org.freedesktop.DBus',interface='org.freedesktop.DBus',\
             member='NameOwnerChanged',arg0='{WATCHER}'"
        );
        let add = Message::call(
            "org.freedesktop.DBus",
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
            "AddMatch",
            vec![Value::Str(rule)],
        );
        bus.call(&add);
        let mut sni = Sni {
            bus,
            name,
            registered: false,
            icons: Vec::new(),
            title: "WeekNumber".into(),
            tooltip: String::new(),
            menu: Vec::new(),
            revision: 1,
        };
        sni.register();
        Some(sni)
    }

    fn register(&mut self) {
        let m = Message::call(
            WATCHER,
            "/StatusNotifierWatcher",
            WATCHER,
            "RegisterStatusNotifierItem",
            vec![Value::Str(self.name.clone())],
        );
        self.registered = self.bus.call(&m).is_some();
    }

    pub fn registered(&self) -> bool {
        self.registered
    }

    pub fn fd(&self) -> std::os::fd::RawFd {
        self.bus.fd()
    }

    /// New icon frames: (side, straight-alpha RGBA pixels).
    pub fn set_icon(&mut self, frames: &[(usize, Vec<[u8; 4]>)]) {
        self.icons = frames
            .iter()
            .map(|(side, px)| {
                let data = px.iter().flat_map(|p| [p[3], p[0], p[1], p[2]]).collect();
                (*side as i32, *side as i32, data)
            })
            .collect();
        self.emit(ITEM_PATH, ITEM_IFACE, "NewIcon", Vec::new());
    }

    pub fn set_tooltip(&mut self, text: &str) {
        if self.tooltip != text {
            self.tooltip = text.to_string();
            self.emit(ITEM_PATH, ITEM_IFACE, "NewToolTip", Vec::new());
        }
    }

    pub fn set_menu(&mut self, items: Vec<MenuItem>) {
        if self.menu != items {
            self.menu = items;
            self.revision += 1;
            let rev = self.revision;
            self.emit(
                MENU_PATH,
                MENU_IFACE,
                "LayoutUpdated",
                vec![Value::U32(rev), Value::I32(0)],
            );
        }
    }

    fn emit(&mut self, path: &str, iface: &str, member: &str, body: Vec<Value>) {
        if self.registered {
            self.bus.send(&Message::signal(path, iface, member, body));
        }
    }

    fn pixmaps(&self) -> Value {
        Value::Array(
            "(iiay)".into(),
            self.icons
                .iter()
                .map(|(w, h, d)| {
                    Value::Struct(vec![
                        Value::I32(*w),
                        Value::I32(*h),
                        Value::Array("y".into(), d.iter().map(|&b| Value::Byte(b)).collect()),
                    ])
                })
                .collect(),
        )
    }

    fn item_property(&self, name: &str) -> Option<Value> {
        Some(match name {
            "Category" => Value::Str("ApplicationStatus".into()),
            "Id" => Value::Str("weeknumber".into()),
            "Title" => Value::Str(self.title.clone()),
            "Status" => Value::Str("Active".into()),
            "WindowId" => Value::I32(0),
            "IconName" | "OverlayIconName" | "AttentionIconName" | "AttentionMovieName"
            | "IconThemePath" => Value::Str(String::new()),
            "IconPixmap" => self.pixmaps(),
            "OverlayIconPixmap" | "AttentionIconPixmap" => {
                Value::Array("(iiay)".into(), Vec::new())
            }
            "ToolTip" => Value::Struct(vec![
                Value::Str(String::new()),
                Value::Array("(iiay)".into(), Vec::new()),
                Value::Str(self.title.clone()),
                Value::Str(self.tooltip.clone()),
            ]),
            "ItemIsMenu" => Value::Bool(false),
            "Menu" => Value::Path(MENU_PATH.into()),
            _ => return None,
        })
    }

    fn menu_property(name: &str) -> Option<Value> {
        Some(match name {
            "Version" => Value::U32(3),
            "TextDirection" => Value::Str("ltr".into()),
            "Status" => Value::Str("normal".into()),
            "IconThemePath" => Value::Array("s".into(), Vec::new()),
            _ => return None,
        })
    }

    fn node_props(&self, id: i32) -> Vec<Value> {
        if id == 0 {
            return vec![Value::entry(
                "children-display",
                Value::Str("submenu".into()),
            )];
        }
        let Some(item) = self.item_by_id(id) else {
            return Vec::new();
        };
        match item {
            MenuItem::Separator => vec![Value::entry("type", Value::Str("separator".into()))],
            MenuItem::Item { label, checked, .. } => {
                let mut v = vec![
                    Value::entry("label", Value::Str(label.replace('_', "__"))),
                    Value::entry("enabled", Value::Bool(true)),
                    Value::entry("visible", Value::Bool(true)),
                ];
                if let Some(c) = checked {
                    v.push(Value::entry("toggle-type", Value::Str("checkmark".into())));
                    v.push(Value::entry("toggle-state", Value::I32(i32::from(*c))));
                }
                v
            }
        }
    }

    fn item_id(i: usize, item: &MenuItem) -> i32 {
        match item {
            MenuItem::Item { id, .. } => *id as i32,
            MenuItem::Separator => SEPARATOR_BASE + i as i32,
        }
    }

    fn item_by_id(&self, id: i32) -> Option<&MenuItem> {
        self.menu
            .iter()
            .enumerate()
            .find(|(i, item)| Sni::item_id(*i, item) == id)
            .map(|(_, item)| item)
    }

    fn layout(&self) -> Value {
        let children: Vec<Value> = self
            .menu
            .iter()
            .enumerate()
            .map(|(i, item)| {
                let id = Sni::item_id(i, item);
                Value::Variant(Box::new(Value::Struct(vec![
                    Value::I32(id),
                    Value::Array("{sv}".into(), self.node_props(id)),
                    Value::Array("v".into(), Vec::new()),
                ])))
            })
            .collect();
        Value::Struct(vec![
            Value::I32(0),
            Value::Array("{sv}".into(), self.node_props(0)),
            Value::Array("v".into(), children),
        ])
    }

    /// Answer what arrived; return what the app should act on.
    pub fn handle(&mut self) -> Vec<TrayEvent> {
        let mut events = Vec::new();
        for m in self.bus.poll() {
            if m.kind == super::dbus::SIGNAL {
                if m.member.as_deref() == Some("NameOwnerChanged")
                    && m.body.first().and_then(Value::as_str) == Some(WATCHER)
                    && m.body
                        .get(2)
                        .and_then(Value::as_str)
                        .is_some_and(|n| !n.is_empty())
                {
                    self.register();
                    if self.registered {
                        events.push(TrayEvent::AboutToShow);
                    }
                }
                continue;
            }
            if m.kind != super::dbus::METHOD_CALL {
                continue;
            }
            let reply = self.answer(&m, &mut events);
            if m.flags & super::dbus::NO_REPLY == 0 {
                self.bus.send(&reply);
            }
        }
        events
    }

    fn answer(&mut self, m: &Message, events: &mut Vec<TrayEvent>) -> Message {
        let unknown = || {
            Message::error_to(
                m,
                "org.freedesktop.DBus.Error.UnknownMethod",
                "No such method",
            )
        };
        let path = m.path.as_deref().unwrap_or("");
        let member = m.member.as_deref().unwrap_or("");
        let iface = m.interface.as_deref().unwrap_or("");
        let arg_str = |i: usize| m.body.get(i).and_then(Value::as_str).unwrap_or("");
        match (iface, member) {
            ("org.freedesktop.DBus.Introspectable", "Introspect") => {
                let xml = match path {
                    ITEM_PATH => INTROSPECT_ITEM,
                    MENU_PATH => INTROSPECT_MENU,
                    _ => "<node><node name=\"StatusNotifierItem\"/><node name=\"MenuBar\"/></node>",
                };
                return Message::reply_to(m, vec![Value::Str(xml.into())]);
            }
            ("org.freedesktop.DBus.Peer", "Ping") => return Message::reply_to(m, Vec::new()),
            _ => {}
        }
        match (path, iface, member) {
            (ITEM_PATH, PROPS_IFACE, "Get") => match self.item_property(arg_str(1)) {
                Some(v) => Message::reply_to(m, vec![Value::Variant(Box::new(v))]),
                None => Message::error_to(
                    m,
                    "org.freedesktop.DBus.Error.UnknownProperty",
                    "No such property",
                ),
            },
            (ITEM_PATH, PROPS_IFACE, "GetAll") => {
                let names = [
                    "Category",
                    "Id",
                    "Title",
                    "Status",
                    "WindowId",
                    "IconName",
                    "IconPixmap",
                    "ToolTip",
                    "ItemIsMenu",
                    "Menu",
                ];
                let all = names
                    .iter()
                    .filter_map(|n| Some(Value::entry(n, self.item_property(n)?)))
                    .collect();
                Message::reply_to(m, vec![Value::Array("{sv}".into(), all)])
            }
            (ITEM_PATH, ITEM_IFACE, "Activate") => {
                let arg = |i: usize| m.body.get(i).and_then(Value::as_i32).unwrap_or(0);
                events.push(TrayEvent::Activate(arg(0), arg(1)));
                Message::reply_to(m, Vec::new())
            }
            (ITEM_PATH, ITEM_IFACE, "SecondaryActivate" | "ContextMenu") => {
                events.push(TrayEvent::ContextMenu);
                Message::reply_to(m, Vec::new())
            }
            (ITEM_PATH, ITEM_IFACE, "Scroll") => Message::reply_to(m, Vec::new()),
            (MENU_PATH, PROPS_IFACE, "Get") => match Sni::menu_property(arg_str(1)) {
                Some(v) => Message::reply_to(m, vec![Value::Variant(Box::new(v))]),
                None => Message::error_to(
                    m,
                    "org.freedesktop.DBus.Error.UnknownProperty",
                    "No such property",
                ),
            },
            (MENU_PATH, PROPS_IFACE, "GetAll") => {
                let all = ["Version", "TextDirection", "Status", "IconThemePath"]
                    .iter()
                    .filter_map(|n| Some(Value::entry(n, Sni::menu_property(n)?)))
                    .collect();
                Message::reply_to(m, vec![Value::Array("{sv}".into(), all)])
            }
            (MENU_PATH, MENU_IFACE, "GetLayout") => {
                Message::reply_to(m, vec![Value::U32(self.revision), self.layout()])
            }
            (MENU_PATH, MENU_IFACE, "GetGroupProperties") => {
                let ids: Vec<i32> = match m.body.first() {
                    Some(Value::Array(_, v)) => v.iter().filter_map(Value::as_i32).collect(),
                    _ => Vec::new(),
                };
                let all: Vec<i32> = if ids.is_empty() {
                    std::iter::once(0)
                        .chain(
                            self.menu
                                .iter()
                                .enumerate()
                                .map(|(i, it)| Sni::item_id(i, it)),
                        )
                        .collect()
                } else {
                    ids
                };
                let list = all
                    .into_iter()
                    .map(|id| {
                        Value::Struct(vec![
                            Value::I32(id),
                            Value::Array("{sv}".into(), self.node_props(id)),
                        ])
                    })
                    .collect();
                Message::reply_to(m, vec![Value::Array("(ia{sv})".into(), list)])
            }
            (MENU_PATH, MENU_IFACE, "GetProperty") => {
                let id = m.body.first().and_then(Value::as_i32).unwrap_or(-1);
                let name = arg_str(1);
                let found = self.node_props(id).into_iter().find_map(|e| match e {
                    Value::Dict(k, v) if k.as_str() == Some(name) => Some(*v),
                    _ => None,
                });
                match found {
                    Some(v) => Message::reply_to(m, vec![v]),
                    None => unknown(),
                }
            }
            (MENU_PATH, MENU_IFACE, "Event") => {
                let id = m.body.first().and_then(Value::as_i32).unwrap_or(-1);
                if arg_str(1) == "clicked"
                    && let Some(MenuItem::Item { id, .. }) = self.item_by_id(id)
                {
                    events.push(TrayEvent::Menu(*id));
                }
                Message::reply_to(m, Vec::new())
            }
            (MENU_PATH, MENU_IFACE, "EventGroup") => {
                if let Some(Value::Array(_, list)) = m.body.first() {
                    for e in list {
                        if let Value::Struct(f) = e
                            && f.get(1).and_then(Value::as_str) == Some("clicked")
                            && let Some(MenuItem::Item { id, .. }) = f
                                .first()
                                .and_then(Value::as_i32)
                                .and_then(|i| self.item_by_id(i))
                        {
                            events.push(TrayEvent::Menu(*id));
                        }
                    }
                }
                Message::reply_to(m, vec![Value::Array("i".into(), Vec::new())])
            }
            (MENU_PATH, MENU_IFACE, "AboutToShow") => {
                events.push(TrayEvent::AboutToShow);
                Message::reply_to(m, vec![Value::Bool(false)])
            }
            (MENU_PATH, MENU_IFACE, "AboutToShowGroup") => {
                events.push(TrayEvent::AboutToShow);
                Message::reply_to(
                    m,
                    vec![
                        Value::Array("i".into(), Vec::new()),
                        Value::Array("i".into(), Vec::new()),
                    ],
                )
            }
            _ => unknown(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_shape() {
        let items = [
            MenuItem::Item {
                id: 1,
                label: "Run at Startup".into(),
                checked: Some(true),
            },
            MenuItem::Separator,
            MenuItem::Item {
                id: 8,
                label: "Exit".into(),
                checked: None,
            },
        ];
        assert_eq!(Sni::item_id(1, &items[1]), SEPARATOR_BASE + 1);
        assert_eq!(Sni::item_id(2, &items[2]), 8);
        let v = Value::Struct(vec![
            Value::I32(0),
            Value::Array("{sv}".into(), Vec::new()),
            Value::Array("v".into(), Vec::new()),
        ]);
        assert_eq!(v.signature(), "(ia{sv}av)");
    }

    /// Against a private bus (dbus-daemon, where installed): a fake
    /// watcher takes the registration, a fake panel reads the icon and the
    /// menu and clicks an item.
    #[test]
    fn on_a_real_bus() {
        use std::io::BufRead;
        let dir = std::env::temp_dir().join(format!("wn-dbus-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let config = dir.join("bus.conf");
        std::fs::write(
            &config,
            format!(
                "<busconfig><type>session</type><listen>unix:path={}/bus</listen>\
                 <policy context=\"default\"><allow send_destination=\"*\" eavesdrop=\"true\"/>\
                 <allow eavesdrop=\"true\"/><allow own=\"*\"/></policy></busconfig>",
                dir.display()
            ),
        )
        .unwrap();
        let Ok(mut daemon) = std::process::Command::new("dbus-daemon")
            .arg(format!("--config-file={}", config.display()))
            .arg("--nofork")
            .arg("--print-address")
            .stdout(std::process::Stdio::piped())
            .spawn()
        else {
            return; // no dbus-daemon here
        };
        let mut line = String::new();
        std::io::BufReader::new(daemon.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        let addr = line.trim().to_string();

        // The watcher: takes its name, accepts one registration.
        let mut watcher = Bus::connect(&addr).expect("watcher connects");
        let own = Message::call(
            "org.freedesktop.DBus",
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
            "RequestName",
            vec![Value::Str(WATCHER.into()), Value::U32(4)],
        );
        assert!(watcher.call(&own).is_some());
        let watcher_thread = std::thread::spawn(move || {
            for _ in 0..100 {
                for m in watcher.poll() {
                    if m.member.as_deref() == Some("RegisterStatusNotifierItem") {
                        watcher.send(&Message::reply_to(&m, Vec::new()));
                        return m.body.first().and_then(Value::as_str).map(str::to_string);
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            None
        });
        let mut sni = Sni::start_on(Bus::connect(&addr).unwrap()).expect("item starts");
        let registered_name = watcher_thread.join().unwrap();
        assert!(sni.registered());
        assert_eq!(registered_name.as_deref(), Some(sni.name.as_str()));
        sni.set_icon(&[(2, vec![[255, 0, 0, 255]; 4])]);
        sni.set_tooltip("Week 43");
        sni.set_menu(vec![
            MenuItem::Item {
                id: 2,
                label: "Bold Text".into(),
                checked: Some(true),
            },
            MenuItem::Separator,
            MenuItem::Item {
                id: 8,
                label: "Exit".into(),
                checked: None,
            },
        ]);

        // The panel: reads properties and the layout, clicks Exit.
        let name = sni.name.clone();
        let panel = std::thread::spawn(move || {
            let mut host = Bus::connect(&addr).unwrap();
            let get = Message::call(
                &name,
                ITEM_PATH,
                PROPS_IFACE,
                "Get",
                vec![
                    Value::Str(ITEM_IFACE.into()),
                    Value::Str("IconPixmap".into()),
                ],
            );
            let icon = host.call(&get).expect("IconPixmap");
            let layout = host
                .call(&Message::call(
                    &name,
                    MENU_PATH,
                    MENU_IFACE,
                    "GetLayout",
                    vec![
                        Value::I32(0),
                        Value::I32(-1),
                        Value::Array("s".into(), Vec::new()),
                    ],
                ))
                .expect("GetLayout");
            let tip = host
                .call(&Message::call(
                    &name,
                    ITEM_PATH,
                    PROPS_IFACE,
                    "Get",
                    vec![Value::Str(ITEM_IFACE.into()), Value::Str("ToolTip".into())],
                ))
                .expect("ToolTip");
            host.call(&Message::call(
                &name,
                MENU_PATH,
                MENU_IFACE,
                "Event",
                vec![
                    Value::I32(8),
                    Value::Str("clicked".into()),
                    Value::Variant(Box::new(Value::I32(0))),
                    Value::U32(0),
                ],
            ));
            host.call(&Message::call(
                &name,
                ITEM_PATH,
                ITEM_IFACE,
                "Activate",
                vec![Value::I32(1), Value::I32(2)],
            ));
            (icon, layout, tip)
        });
        let mut events = Vec::new();
        for _ in 0..200 {
            events.extend(sni.handle());
            if events.contains(&TrayEvent::Activate(1, 2)) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let (icon, layout, tip) = panel.join().unwrap();
        assert!(events.contains(&TrayEvent::Menu(8)), "{events:?}");
        let Value::Variant(icon) = &icon.body[0] else {
            panic!()
        };
        assert_eq!(icon.signature(), "a(iiay)");
        let Value::Array(_, frames) = icon.as_ref() else {
            panic!()
        };
        let Value::Struct(f) = &frames[0] else {
            panic!()
        };
        // ARGB, network byte order.
        assert_eq!(
            f[2],
            Value::Array(
                "y".into(),
                [255, 255, 0, 0]
                    .repeat(4)
                    .into_iter()
                    .map(Value::Byte)
                    .collect()
            )
        );
        assert_eq!(layout.body[1].signature(), "(ia{sv}av)");
        let Value::Struct(root) = &layout.body[1] else {
            panic!()
        };
        let Value::Array(_, children) = &root[2] else {
            panic!()
        };
        assert_eq!(children.len(), 3);
        let Value::Variant(tip) = &tip.body[0] else {
            panic!()
        };
        assert_eq!(tip.signature(), "(sa(iiay)ss)");
        let _ = daemon.kill();
        let _ = daemon.wait();
        let _ = std::fs::remove_dir_all(dir);
    }
}
