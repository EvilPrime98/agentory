use gpui::App;

#[cfg(windows)]
mod win {
    use std::path::PathBuf;
    use std::sync::OnceLock;

    use futures_channel::mpsc::UnboundedSender;
    use windows::Data::Xml::Dom::XmlDocument;
    use windows::Foundation::TypedEventHandler;
    use windows::UI::Notifications::{ToastNotification, ToastNotificationManager};
    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, REG_SZ, RegSetKeyValueW};
    use windows::core::{HSTRING, IInspectable, PCWSTR};

    const AUMID: &str = "Agentory";
    const ICON: &[u8] = include_bytes!("../../packaging/icon.png");

    static CLICKS: OnceLock<UnboundedSender<String>> = OnceLock::new();
    static ICON_PATH: OnceLock<Option<PathBuf>> = OnceLock::new();

    pub fn set_clicks(tx: UnboundedSender<String>) {
        let _ = CLICKS.set(tx);
    }

    fn set_value(subkey: &str, name: &str, value: &str) -> windows::core::Result<()> {
        let subkey = HSTRING::from(subkey);
        let name = HSTRING::from(name);
        let data: Vec<u16> = value.encode_utf16().chain(Some(0)).collect();
        unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                PCWSTR(subkey.as_ptr()),
                PCWSTR(name.as_ptr()),
                REG_SZ.0,
                Some(data.as_ptr().cast()),
                (data.len() * 2) as u32,
            )
            .ok()
        }
    }

    fn icon_path() -> Option<&'static PathBuf> {
        ICON_PATH
            .get_or_init(|| {
                let path = dirs::data_local_dir()?.join("agentory").join("icon.png");
                std::fs::create_dir_all(path.parent()?).ok()?;
                std::fs::write(&path, ICON).ok()?;
                Some(path)
            })
            .as_ref()
    }

    pub fn register() {
        let key = format!(r"Software\Classes\AppUserModelId\{AUMID}");
        let mut res = set_value(&key, "DisplayName", "Agentory");
        if let Some(icon) = icon_path() {
            res = res.and(set_value(&key, "IconUri", &icon.to_string_lossy()));
        }
        if let Err(e) = res {
            tracing::warn!("toast identity registration failed: {e}");
        }
    }

    fn escape(s: &str) -> String {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
            .replace('\'', "&apos;")
    }

    pub fn show(title: &str, body: &str, session_id: &str) -> windows::core::Result<()> {
        let logo = icon_path()
            .map(|p| {
                let uri = p.to_string_lossy().replace('\\', "/");
                format!(
                    "<image placement='appLogoOverride' src='file:///{}'/>",
                    escape(&uri)
                )
            })
            .unwrap_or_default();
        let xml = format!(
            "<toast><visual><binding template='ToastGeneric'>{logo}<text>{}</text><text>{}</text></binding></visual></toast>",
            escape(title),
            escape(body)
        );
        let doc = XmlDocument::new()?;
        doc.LoadXml(&HSTRING::from(xml))?;
        let toast = ToastNotification::CreateToastNotification(&doc)?;
        let tag: String = session_id.chars().take(64).collect();
        toast.SetTag(&HSTRING::from(tag))?;
        toast.SetGroup(&HSTRING::from("sessions"))?;
        let id = session_id.to_string();
        toast.Activated(&TypedEventHandler::new(
            move |_: windows::core::Ref<ToastNotification>, _: windows::core::Ref<IInspectable>| {
                if let Some(tx) = CLICKS.get() {
                    let _ = tx.unbounded_send(id.clone());
                }
                Ok(())
            },
        ))?;
        ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(AUMID))?.Show(&toast)
    }
}

pub fn init(cx: &mut App, on_click: impl Fn(&str, &mut App) + 'static) {
    #[cfg(windows)]
    {
        use futures_util::StreamExt;

        win::register();
        let (tx, mut rx) = futures_channel::mpsc::unbounded::<String>();
        win::set_clicks(tx);
        cx.spawn(async move |cx| {
            while let Some(id) = rx.next().await {
                let _ = cx.update(|cx| on_click(&id, cx));
            }
        })
        .detach();
    }
    #[cfg(not(windows))]
    {
        let _ = (cx, on_click);
    }
}

pub fn toast(title: &str, body: &str, session_id: &str) {
    #[cfg(windows)]
    if let Err(e) = win::show(title, body, session_id) {
        tracing::warn!("toast failed: {e}");
    }
    #[cfg(not(windows))]
    {
        let _ = (title, body, session_id);
    }
}
