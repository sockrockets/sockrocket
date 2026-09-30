pub fn lookup(key: &str) -> Option<&'static str> {
    Some(match key {
        "chrome.brand" => "Rocket",
        "nav.dashboard" => "Dashboard",
        "nav.nodes" => "Nodes",
        "nav.groups" => "Groups",
        "nav.connections" => "Connections",
        "nav.rules" => "Rules",
        "nav.logs" => "Logs",
        "nav.settings" => "Settings",
        "settings.title" => "Settings",
        "settings.section.proxy" => "Proxy",
        "settings.listen_address" => "Listen Address",
        "settings.socks_port" => "SOCKS5 Port",
        "settings.http_port" => "HTTP Port",
        "settings.apply" => "Apply Changes",
        "settings.listeners_restart_hint" => {
            "Listen address and port changes restart the local proxy listeners."
        }
        "settings.section.system" => "System",
        "settings.system_proxy" => "System Proxy",
        "settings.theme" => "Theme",
        "settings.theme.dark" => "Dark",
        "settings.language" => "Language",
        "settings.section.about" => "About",
        "settings.about.name" => "Sockrocket Proxy Client",
        "settings.about.github" => "GitHub",
        "settings.about.check_update" => "Check for Updates",
        "settings.about.open_download" => "Download Update",
        "settings.about.update_keep_config" => "Your settings, nodes, subscriptions, and rules stay in the user data folder — only the app binary is replaced.",
        _ => return None,
    })
}
