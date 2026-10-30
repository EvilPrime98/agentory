//! Windows toast notifications, through PowerShell's WinRT bridge (no extra crate). Best effort:
//! failures are logged and never surface to the user.

/// Show a toast. Title/body travel in environment variables so they never need quoting.
pub async fn toast(title: String, body: String) {
    #[cfg(windows)]
    {
        const SCRIPT: &str = r#"
[Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] | Out-Null
[Windows.Data.Xml.Dom.XmlDocument, Windows.Data.Xml.Dom.XmlDocument, ContentType = WindowsRuntime] | Out-Null
$xml = New-Object Windows.Data.Xml.Dom.XmlDocument
$t = [System.Security.SecurityElement]::Escape($env:AGENTORY_TOAST_TITLE)
$b = [System.Security.SecurityElement]::Escape($env:AGENTORY_TOAST_BODY)
$xml.LoadXml("<toast><visual><binding template='ToastGeneric'><text>$t</text><text>$b</text></binding></visual></toast>")
$toast = New-Object Windows.UI.Notifications.ToastNotification $xml
[Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier('{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\WindowsPowerShell\v1.0\powershell.exe').Show($toast)
"#;
        let result = tokio::process::Command::new("powershell")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-WindowStyle",
                "Hidden",
                "-Command",
                SCRIPT,
            ])
            .env("AGENTORY_TOAST_TITLE", &title)
            .env("AGENTORY_TOAST_BODY", &body)
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
            .kill_on_drop(true)
            .output()
            .await;
        match result {
            Ok(out) if out.status.success() => {}
            Ok(out) => tracing::warn!(
                "toast failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ),
            Err(e) => tracing::warn!("toast failed: {e}"),
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (title, body);
    }
}
