pub fn lookup(key: &str) -> Option<&'static str> {
    Some(match key {
        "chrome.brand" => "Rocket",
        "nav.dashboard" => "Bảng điều khiển",
        "nav.nodes" => "Nút",
        "nav.groups" => "Nhóm",
        "nav.connections" => "Kết nối",
        "nav.rules" => "Quy tắc",
        "nav.logs" => "Nhật ký",
        "nav.settings" => "Cài đặt",
        "settings.title" => "Cài đặt",
        "settings.section.proxy" => "Proxy",
        "settings.listen_address" => "Địa chỉ lắng nghe",
        "settings.socks_port" => "Cổng SOCKS5",
        "settings.http_port" => "Cổng HTTP",
        "settings.apply" => "Áp dụng",
        "settings.listeners_restart_hint" => {
            "Thay đổi địa chỉ hoặc cổng lắng nghe sẽ khởi động lại proxy cục bộ."
        }
        "settings.section.system" => "Hệ thống",
        "settings.system_proxy" => "Proxy hệ thống",
        "settings.theme" => "Giao diện",
        "settings.theme.dark" => "Tối",
        "settings.language" => "Ngôn ngữ",
        "settings.section.about" => "Giới thiệu",
        "settings.about.name" => "Ứng dụng proxy Sockrocket",
        "settings.about.github" => "GitHub",
        _ => return None,
    })
}
