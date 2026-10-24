'use strict';
const translations = {
en: {
skip:'Skip to content',navExperience:'Experience',navDownload:'Download',navDocs:'Docs',navCommunity:'Community',heroLabel:'Native client · Open-source Rust engine',heroTitle:'From your desktop.<br>To your router.',heroDescription:'Nodes, subscriptions, rules and connection status, clearly in one place. One proxy engine, across your devices.',heroDownload:'Download Sockrocket',heroGuide:'Read the guide',heroPlatforms:'macOS · Windows · Linux · AsusWRT-Merlin',heroWindow:'Familiar layout. Clear controls.',imageNote:'Design illustration · Synthetic data; released interfaces may differ',floatTitle:'Connection, with a way back',floatText:'Saved configuration · Proxy and route recovery',protocolLabel:'One engine. The protocols you know.',experienceLabel:'LESS SEARCHING. MORE CONTROL.',experienceTitle:'A place for every setting.',experienceDescription:'A grouped layout inspired by macOS System Settings, with straightforward everyday controls and details within reach.',tabNodes:'Nodes & subscriptions',tabNodesSub:'Clear sources. Simple selection.',tabRules:'Rules & routing',tabRulesSub:'Inspect, edit and choose an exit.',tabBulk:'Bulk rule editing',tabBulkSub:'Paste, review, then save.',workspaceNote:'Rule, Global or Direct.<br>Choose for your use case.',capOneTitle:'Choose how to capture traffic',capOneText:'System proxy for applications that follow proxy settings; TUN when you need broader traffic capture.',capTwoTitle:'See the connection details',capTwoText:'Inspect the selected exit, connections, traffic and logs instead of relying on a Connected label alone.',capThreeTitle:'Keep your configuration locally',capThreeText:'Subscription URLs, nodes and selection are saved. Sockrocket does not provide nodes or subscriptions.',merlinLabel:'SOCKROCKET FOR MERLIN',merlinTitle:'Your router.<br>More connected devices.',merlinDescription:'Manage nodes, subscriptions and routing in the router WebUI. Transparent proxying and DNS interception bring the same engine to LAN devices.',merlinGuide:'Merlin installation guide',merlinNote:'Packages must match firmware platform and CPU architecture. Check the guide for compatibility.',routerOne:'Nodes & subscriptions',routerTwo:'Transparent routing',routerThree:'Name resolution',routerBottom:'One engine. Multiple devices.',networkNote:'Architecture illustration, not live network status.',downloadLabel:'CHOOSE YOUR STARTING POINT',downloadTitle:'Start on the device you use.',downloadDescription:'All release files are on GitHub Releases. Choose your OS and architecture; CLI and Merlin packages are on the same page.',downloadMac:'Download .dmg ↗',downloadWindows:'Download .zip ↗',downloadLinux:'Download .tar.gz ↗',downloadRouterSub:'Match your firmware platform',downloadRouter:'Choose a router package ↗',downloadNote:'Quit the old version and its background instance before upgrading to avoid competing ports or routes.',releaseHistory:'All releases & changelog ↗',startLabel:'GETTING STARTED',startTitle:'Three steps to your connection.',startGuide:'Complete getting-started guide →',stepOneTitle:'Add your subscription or nodes',stepOneText:'Import node links or Clash, V2Ray and sing-box subscriptions.',stepTwoTitle:'Select a node and review routing',stepTwoText:'Test latency, then choose Rule, Global or Direct mode for your needs.',stepThreeTitle:'Connect and verify access',stepThreeText:'Verify the system proxy first. Enable TUN and authorize it when broader capture is needed.',cliTitle:'Or start from the terminal.',cliGuide:'Configuration guide →',copy:'Copy commands',copied:'Copied',copyFailed:'Select and copy the commands manually',cliNote:'Before connecting, add your own nodes or subscriptions to the local configuration file.',faqTitle:'A few things to know first.',faqOneQ:'Does Sockrocket provide proxy nodes?',faqOneA:'No. Sockrocket is a proxy client and engine. You need your own working nodes or subscription.',faqTwoQ:'System proxy or TUN?',faqTwoA:'System proxy covers applications that follow OS proxy settings. TUN captures more traffic and requires administrator privileges and a working driver. Avoid running multiple TUNs or VPNs together.',faqThreeQ:'Which Merlin package should I choose?',faqThreeA:'Match the firmware platform tag and CPU architecture, rather than guessing from the router name. Read the Merlin guide, back up configuration and check JFFS space before upgrading.',faqFourQ:'What should I include in a bug report?',faqFourA:'Include software and OS/firmware versions, reproduction steps and sanitized logs. Remove full subscription URLs, node addresses, passwords, UUIDs, tokens, QR codes and personal paths.',communityLabel:'BUILD BETTER CONNECTIONS, TOGETHER',communityTitle:'Updates. Questions. Feedback.<br>Find us here.',telegramDescription:'Release announcements & updates · @sockrocket',discussionsDescription:'Usage questions, ideas and conversations',issuesDescription:'Bug reports with sanitized reproduction steps',footer:'Open-source Rust proxy client · Apache-2.0',docs:'Docs'
},
vi: {
skip:'Đến nội dung',navExperience:'Trải nghiệm',navDownload:'Tải xuống',navDocs:'Tài liệu',navCommunity:'Cộng đồng',heroLabel:'Ứng dụng native · Lõi Rust mã nguồn mở',heroTitle:'Từ máy tính.<br>Đến router.',heroDescription:'Node, subscription, quy tắc và trạng thái kết nối, rõ ràng trong cùng một nơi. Một lõi proxy cho nhiều thiết bị.',heroDownload:'Tải Sockrocket',heroGuide:'Đọc hướng dẫn',heroPlatforms:'macOS · Windows · Linux · AsusWRT-Merlin',heroWindow:'Bố cục quen thuộc. Điều khiển rõ ràng.',imageNote:'Minh họa thiết kế · Dữ liệu giả; giao diện phát hành có thể khác',floatTitle:'Kết nối và khôi phục',floatText:'Lưu cấu hình · Khôi phục proxy và tuyến mạng',protocolLabel:'Một lõi. Những giao thức quen thuộc.',experienceLabel:'DỄ TÌM. DỄ ĐIỀU KHIỂN.',experienceTitle:'Mỗi cài đặt, một vị trí rõ ràng.',experienceDescription:'Bố cục nhóm theo phong cách macOS System Settings, đơn giản cho thao tác thường ngày và dễ tìm chi tiết.',tabNodes:'Node & subscription',tabNodesSub:'Nguồn rõ ràng, dễ lựa chọn.',tabRules:'Quy tắc & định tuyến',tabRulesSub:'Xem, chỉnh sửa, chọn lối ra.',tabBulk:'Sửa quy tắc hàng loạt',tabBulkSub:'Dán, kiểm tra, rồi lưu.',workspaceNote:'Rule, Global hoặc Direct.<br>Chọn theo nhu cầu sử dụng.',capOneTitle:'Chọn cách chuyển lưu lượng',capOneText:'Proxy hệ thống cho ứng dụng tuân theo cài đặt proxy; TUN khi cần chuyển nhiều lưu lượng hơn.',capTwoTitle:'Xem chi tiết kết nối',capTwoText:'Kiểm tra lối ra, kết nối, lưu lượng và nhật ký thay vì chỉ dựa vào nhãn Đã kết nối.',capThreeTitle:'Lưu cấu hình trên thiết bị',capThreeText:'URL subscription, node và lựa chọn được lưu. Sockrocket không cung cấp node hay subscription.',merlinLabel:'SOCKROCKET FOR MERLIN',merlinTitle:'Một router.<br>Kết nối nhiều thiết bị.',merlinDescription:'Quản lý node, subscription và định tuyến trong WebUI của router. Proxy trong suốt và DNS đưa cùng lõi đến các thiết bị LAN.',merlinGuide:'Hướng dẫn cài Merlin',merlinNote:'Gói cài đặt phải khớp nền tảng firmware và kiến trúc CPU. Xem hướng dẫn để kiểm tra tương thích.',routerOne:'Node & subscription',routerTwo:'Định tuyến trong suốt',routerThree:'Phân giải tên miền',routerBottom:'Một lõi. Nhiều thiết bị.',networkNote:'Minh họa kiến trúc, không phải trạng thái mạng trực tiếp.',downloadLabel:'CHỌN ĐIỂM BẮT ĐẦU',downloadTitle:'Bắt đầu trên thiết bị của bạn.',downloadDescription:'Tất cả tệp phát hành nằm trên GitHub Releases. Chọn hệ điều hành và kiến trúc; CLI và gói Merlin cũng ở cùng trang.',downloadMac:'Tải .dmg ↗',downloadWindows:'Tải .zip ↗',downloadLinux:'Tải .tar.gz ↗',downloadRouterSub:'Chọn đúng nền tảng firmware',downloadRouter:'Chọn gói cho router ↗',downloadNote:'Thoát phiên bản cũ và tiến trình nền trước khi nâng cấp để tránh xung đột cổng hoặc tuyến mạng.',releaseHistory:'Các phiên bản & thay đổi ↗',startLabel:'BẮT ĐẦU',startTitle:'Ba bước để kết nối.',startGuide:'Hướng dẫn sử dụng đầy đủ →',stepOneTitle:'Thêm subscription hoặc node của bạn',stepOneText:'Nhập liên kết node hoặc subscription Clash, V2Ray và sing-box.',stepTwoTitle:'Chọn node và kiểm tra định tuyến',stepTwoText:'Đo độ trễ, rồi chọn chế độ Rule, Global hoặc Direct theo nhu cầu.',stepThreeTitle:'Kết nối và kiểm tra truy cập',stepThreeText:'Kiểm tra proxy hệ thống trước. Bật TUN và cấp quyền khi cần chuyển thêm lưu lượng.',cliTitle:'Hoặc bắt đầu từ terminal.',cliGuide:'Hướng dẫn cấu hình →',copy:'Sao chép lệnh',copied:'Đã sao chép',copyFailed:'Vui lòng chọn và sao chép lệnh thủ công',cliNote:'Trước khi kết nối, thêm node hoặc subscription của bạn vào tệp cấu hình trên máy.',faqTitle:'Một vài điều cần biết trước.',faqOneQ:'Sockrocket có cung cấp node không?',faqOneA:'Không. Sockrocket là ứng dụng và lõi proxy. Bạn cần có node hoặc subscription hoạt động của riêng mình.',faqTwoQ:'Proxy hệ thống hay TUN?',faqTwoA:'Proxy hệ thống dành cho ứng dụng tuân theo cài đặt proxy của OS. TUN chuyển nhiều lưu lượng hơn và cần quyền quản trị cùng driver phù hợp. Tránh chạy nhiều TUN hoặc VPN đồng thời.',faqThreeQ:'Nên chọn gói Merlin nào?',faqThreeA:'Chọn theo nhãn nền tảng firmware và kiến trúc CPU, không chỉ theo tên router. Đọc hướng dẫn Merlin, sao lưu cấu hình và kiểm tra dung lượng JFFS trước khi nâng cấp.',faqFourQ:'Cần gửi gì khi báo lỗi?',faqFourA:'Gửi phiên bản phần mềm, OS/firmware, bước tái hiện và nhật ký đã ẩn thông tin. Xóa URL subscription đầy đủ, địa chỉ node, mật khẩu, UUID, token, mã QR và đường dẫn cá nhân.',communityLabel:'CÙNG NHAU CẢI THIỆN KẾT NỐI',communityTitle:'Cập nhật. Hỏi đáp. Góp ý.<br>Liên hệ tại đây.',telegramDescription:'Thông báo phát hành & cập nhật · @sockrocket',discussionsDescription:'Hỏi đáp, ý tưởng và trao đổi sử dụng',issuesDescription:'Báo lỗi với bước tái hiện đã ẩn thông tin',footer:'Ứng dụng proxy Rust mã nguồn mở · Apache-2.0',docs:'Tài liệu'
}};
Object.assign(translations.en, {
 merlinScreenshotNote:'Actual Merlin interface · Nodes, addresses, SSIDs and credentials hidden; metrics reflect capture time',
 protocolLabel:'One Rust engine. Protocol capabilities in source.',
 protocolUpdateLabel:'PROTOCOL EXPANSION · CURRENT SOURCE',
 protocolUpdateTitle:'Two more native options.',
 protocolSourceBadge:'Source capabilities · See release notes',
 protocolUpdateIntro:'AnyTLS and WireGuard are integrated with desktop, CLI and Merlin. These capabilities describe the current source; downloaded packages follow their own release notes.',
 anytlsDescription:'Native TLS connections, session reuse and UDP-over-TCP v2. Import share links, Clash or sing-box configurations. Server certificates are verified by default.',
 anytlsDetail:'TCP and UDP interoperability verified against an independent server.',
 wireguardDescription:'A native userspace tunnel with one peer per node. Supports preshared keys, DNS inside the tunnel and WireGuard profile import.',
 wireguardDetail:'No additional OS interface; UDP payloads must fit the tunnel MTU.',
 protocolLimits:'Compatibility: VLESS Vision supports TLS 1.3 / REALITY over TCP and XUDP (standard flow excludes UDP/443). gRPC / XHTTP remain unsupported and fail explicitly. WireGuard does not yet support multiple peers, IP fragmentation or profile scripts.',
 protocolGuide:'Protocol and transport guide →',
 heroLabel:'AsusWRT-Merlin · Open-source Rust engine',
 heroTitle:'One router.<br>The whole home.',
 heroDescription:'Install the same engine on Merlin. Computers, phones and tablets on the LAN share nodes, rules and DNS. The desktop client is still there.',
 heroDownload:'Download the router plugin',
 heroGuide:'Read the install guide',
 heroPlatforms:'Match the firmware platform · hnd_v8 / hnd / arm / qca / mtk',
 heroWindow:'The desktop client uses the same engine.',
 merlinNote:'The package must match the firmware platform and CPU. Compatibility stays the default, and it does not require ISP IPv6.',
 merlinFirmware:'No Software Center yet? Download a firmware that includes it, then flash →',
 pkgHndV8:'hnd_v8 · AX86U, AX88U, GT-AX6000',
 pkgHnd:'hnd · AX58U, XT8, TUF-AX3000',
 pkgArm:'arm · AC68U, AC88U, AC5300',
 merlinAllPlatforms:'Every platform, checksums and desktop builds',
 downloadLabel:'DIRECT DOWNLOAD',
 downloadTitle:'Click a filename to download.',
 downloadDescription:'Each link is that file on the latest release. Pick the router package by firmware platform, not by the marketing name.',
 downloadVersionLabel:'Current release',
 downloadBaseLabel:'Latest release directory',
 routerDownloadsTitle:'Router · AsusWRT-Merlin',
 routerDownloadsLead:'Install the matching tar.gz from Software Center. The wrong platform fails with Exec format error, especially hnd versus hnd_v8.',
 firmwareLead:'If the menu has no Software Center, download firmware from <a href="https://fw.koolcenter.com/">fw.koolcenter.com</a> first: Merlin builds 3004.388 and 3006.102, or the Asus official-mod builds. Open the folder named like your model, take the newest stable file, and upgrade it under Administration. After reboot, enable JFFS, then offline-install a package below. Steps: <a href="docs/merlin.html#firmware">install guide</a>.',
 colPlatform:'Platform',
 colModels:'Typical models',
 colSystem:'System',
 colFile:'File',
 pkgHndV8Models:'RT-AX86U / PRO, RT-AX86S, RT-AX88U / PRO, RT-AX68U, RT-AX92U, GT-AX6000, GT-AX11000 / PRO, GT-AXE11000, GT-AXE16000, ZenWiFi Pro XT12, RT-AC86U, GT-AC2900, GT-AC5300. BCM490x / BCM491x. Co-branded editions use the same package as the base model.',
 pkgHndModels:'TUF-AX3000 / V2, TUF-AX5400, RT-AX58U / V2, RT-AX82U / V2, RT-AX56U, RT-AX55, RT-AX57, ZenWiFi XT8, ZenWiFi XD4. BCM675x.',
 pkgArmModels:'RT-AC68U, RT-AC66U B1, RT-AC1900 / P, RT-AC87U, RT-AC88U, RT-AC3100, RT-AC3200, RT-AC5300. BCM4708 / BCM4709, on Merlin with Software Center.',
 pkgQcaModels:'RT-AX89X, RT-AC89X. IPQ807x.',
 pkgMtkModels:'TUF-AX4200, TUF-AX6000, RT-AX59U, ZenWiFi BD4. MT798x.',
 pkgIpq32Models:'IPQ53xx when uname -m is armv7l. The same chassis on 64-bit firmware uses ipq64.',
 pkgIpq64Models:'IPQ53xx when uname -m is aarch64. The same chassis on 32-bit firmware uses ipq32.',
 faqThreeA:'Match the models in the download table. Co-branded editions use the same package as the base model. On IPQ53xx, check uname -m: armv7l uses ipq32, aarch64 uses ipq64. Read the Merlin guide, then back up the configuration and check JFFS space before upgrading.',
 checksumHint:' · checksum for the router packages',
 merlinGuideLink:'Install steps',
 desktopDownloadsTitle:'Desktop and command line',
 colKind:'Use',
 kindGui:'Desktop',
 kindCli:'Command line',
 kindRouterBin:'Router binary',
 binAarch64:'aarch64, manual replace; not a substitute for the package above',
 binArmv7:'armv7 hardfloat, manual replace',
 binArmv7sf:'armv7 softfloat, manual replace',
 downloadNote:'Install a router package from Software Center. Quit the old desktop app and its background instance before upgrading, so ports and routes are not claimed twice.',
 navLater:'Features',
 laterLabel:'ALREADY ON THE ROUTER',
 laterTitle:'Groups, exits and traffic, each with one job.',
 laterDescription:'These were added to the Merlin pages later. The desktop app shares the same core. The notes below follow the router.',
 laterKindGroup:'GROUP',
 laterGroupTitle:'Node groups',
 laterGroupText:'Manual selection uses one node. Fallback tries the list in order and, for five minutes, starts at the last success. URL test keeps the faster handshake for five minutes. Order does not pick the winner.',
 laterKindExit:'EXIT',
 laterExitTitle:'Rule exits',
 laterExitText:'The global node follows the node selected at the top. A single node stays pinned. A node group is its own exit and does not change the global node. Only Rule mode reads these exits.',
 laterKindTraffic:'TRAFFIC',
 laterTrafficTitle:'Traffic',
 laterTrafficText:'Overview counts bytes since this process started and resets on restart. Subscriptions show the last three calendar months. A subscription total is the sum of its current nodes. Addresses are not shown.',
 laterKindTrace:'CHECK',
 laterTraceTitle:'Routing check',
 laterTraceText:'Enter a site on the routing page and see which rule matched and which exit it uses. This explains saved rules. It does not open a proxy connection or store the query.',
 laterKindPreset:'PRESETS',
 laterPresetTitle:'Rule presets',
 laterPresetText:'AI, overseas video, Telegram web, developer tools, Apple, Microsoft and domestic sites. They are added disabled. The lists are common domains, not every endpoint.',
 laterKindNet:'NETWORK',
 laterNetTitle:'IPv6 and switches',
 laterNetText:'Compatibility is the default and does not need ISP IPv6. Start turns DNS interception and transparent proxy on together. Either can be turned off while running. Turning transparent proxy on also turns DNS interception on.',
 laterRouting:'How routing takes effect →',
 laterConfig:'Traffic and DNS →',
 faqFiveQ:'I created a group. Why is traffic not using it?',
 faqFiveA:'A group does not take over by itself. Leave Overview in Rule mode, then set the routing-rule exit to Group · name. Direct and reject rules stay. Global and Direct modes ignore groups.',
 faqSixQ:'Why does a subscription total not match one node?',
 faqSixA:'The total adds the nodes that currently belong to that subscription. Direct traffic and nodes with no subscription stay in the node table. The page does not show subscription addresses.'
});
Object.assign(translations.vi, {
 merlinScreenshotNote:'Giao diện Merlin thực tế · Đã ẩn node, địa chỉ, SSID và thông tin xác thực; số liệu tại thời điểm chụp',
 protocolLabel:'Một lõi Rust. Khả năng giao thức trong mã nguồn.',
 protocolUpdateLabel:'MỞ RỘNG GIAO THỨC · MÃ NGUỒN HIỆN TẠI',
 protocolUpdateTitle:'Thêm hai lựa chọn native.',
 protocolSourceBadge:'Khả năng mã nguồn · Xem ghi chú phát hành',
 protocolUpdateIntro:'AnyTLS và WireGuard đã được tích hợp vào desktop, CLI và Merlin. Nội dung này mô tả mã nguồn hiện tại; các gói tải xuống tuân theo ghi chú của từng bản phát hành.',
 anytlsDescription:'Kết nối TLS native, tái sử dụng phiên và UDP-over-TCP v2. Nhập liên kết, cấu hình Clash hoặc sing-box. Mặc định xác minh chứng chỉ máy chủ.',
 anytlsDetail:'Đã kiểm tra tương thích TCP và UDP với máy chủ độc lập.',
 wireguardDescription:'Đường hầm native trong tiến trình, mỗi node có một peer. Hỗ trợ khóa chia sẻ trước, DNS trong đường hầm và nhập tệp cấu hình WireGuard.',
 wireguardDetail:'Không tạo thêm giao diện mạng hệ thống; gói UDP phải nằm trong MTU của đường hầm.',
 protocolLimits:'Giới hạn: VLESS Vision hỗ trợ TLS 1.3 / REALITY qua TCP và XUDP (flow tiêu chuẩn chặn UDP/443). gRPC / XHTTP chưa được triển khai và sẽ bị từ chối rõ ràng. WireGuard chưa hỗ trợ nhiều peer, phân mảnh IP hoặc script cấu hình.',
 protocolGuide:'Hướng dẫn giao thức và transport →',
 heroLabel:'AsusWRT-Merlin · Lõi Rust mã nguồn mở',
 heroTitle:'Một router.<br>Cả nhà cùng dùng.',
 heroDescription:'Cài cùng một lõi trên Merlin. Máy tính, điện thoại và máy tính bảng trong LAN dùng chung node, quy tắc và DNS. Ứng dụng desktop vẫn có.',
 heroDownload:'Tải plugin cho router',
 heroGuide:'Đọc hướng dẫn cài đặt',
 heroPlatforms:'Chọn đúng nền tảng firmware · hnd_v8 / hnd / arm / qca / mtk',
 heroWindow:'Ứng dụng desktop dùng cùng một lõi.',
 merlinNote:'Gói phải khớp nền tảng firmware và CPU. Mặc định giữ tương thích IPv4, không yêu cầu ISP có IPv6.',
 merlinFirmware:'Chưa có Software Center? Tải firmware có sẵn nó, rồi flash →',
 pkgHndV8:'hnd_v8 · AX86U, AX88U, GT-AX6000',
 pkgHnd:'hnd · AX58U, XT8, TUF-AX3000',
 pkgArm:'arm · AC68U, AC88U, AC5300',
 merlinAllPlatforms:'Mọi nền tảng, checksum và bản desktop',
 downloadLabel:'TẢI TRỰC TIẾP',
 downloadTitle:'Bấm tên tệp để tải.',
 downloadDescription:'Mỗi liên kết là đúng tệp đó trong bản phát hành mới nhất. Gói router chọn theo nền tảng firmware, không theo tên tiếp thị.',
 downloadVersionLabel:'Phiên bản hiện tại',
 downloadBaseLabel:'Thư mục bản mới nhất',
 routerDownloadsTitle:'Router · AsusWRT-Merlin',
 routerDownloadsLead:'Cài tar.gz tương ứng từ Software Center. Sai nền tảng sẽ gặp Exec format error, đặc biệt là hnd và hnd_v8.',
 firmwareLead:'Nếu menu chưa có Software Center, hãy tải firmware tại <a href="https://fw.koolcenter.com/">fw.koolcenter.com</a>: bản Merlin 3004.388, 3006.102, hoặc bản mod chính hãng Asus. Vào thư mục trùng tên model, lấy tệp ổn định mới nhất và nâng cấp trong Administration. Sau khi khởi động lại, bật JFFS rồi cài offline gói bên dưới. Các bước: <a href="docs/merlin.html#firmware">hướng dẫn cài</a>.',
 colPlatform:'Nền tảng',
 colModels:'Máy thường gặp',
 colSystem:'Hệ thống',
 colFile:'Tệp',
 pkgHndV8Models:'RT-AX86U / PRO, RT-AX86S, RT-AX88U / PRO, RT-AX68U, RT-AX92U, GT-AX6000, GT-AX11000 / PRO, GT-AXE11000, GT-AXE16000, ZenWiFi Pro XT12, RT-AC86U, GT-AC2900, GT-AC5300. BCM490x / BCM491x. Bản liên danh dùng cùng gói với model gốc.',
 pkgHndModels:'TUF-AX3000 / V2, TUF-AX5400, RT-AX58U / V2, RT-AX82U / V2, RT-AX56U, RT-AX55, RT-AX57, ZenWiFi XT8, ZenWiFi XD4. BCM675x.',
 pkgArmModels:'RT-AC68U, RT-AC66U B1, RT-AC1900 / P, RT-AC87U, RT-AC88U, RT-AC3100, RT-AC3200, RT-AC5300. BCM4708 / BCM4709, cần Merlin có Software Center.',
 pkgQcaModels:'RT-AX89X, RT-AC89X. IPQ807x.',
 pkgMtkModels:'TUF-AX4200, TUF-AX6000, RT-AX59U, ZenWiFi BD4. MT798x.',
 pkgIpq32Models:'IPQ53xx khi uname -m là armv7l. Cùng vỏ máy nhưng firmware 64-bit thì dùng ipq64.',
 pkgIpq64Models:'IPQ53xx khi uname -m là aarch64. Cùng vỏ máy nhưng firmware 32-bit thì dùng ipq32.',
 faqThreeA:'Đối chiếu máy thường gặp trong bảng tải xuống. Bản liên danh dùng cùng gói với model gốc. IPQ53xx xem uname -m: armv7l dùng ipq32, aarch64 dùng ipq64. Đọc hướng dẫn Merlin, sao lưu cấu hình và kiểm tra dung lượng JFFS trước khi nâng cấp.',
 checksumHint:' · checksum cho các gói router',
 merlinGuideLink:'Các bước cài',
 desktopDownloadsTitle:'Desktop và dòng lệnh',
 colKind:'Mục đích',
 kindGui:'Desktop',
 kindCli:'Dòng lệnh',
 kindRouterBin:'Binary cho router',
 binAarch64:'aarch64, thay thủ công; không thay cho gói cài ở trên',
 binArmv7:'armv7 hardfloat, thay thủ công',
 binArmv7sf:'armv7 softfloat, thay thủ công',
 downloadNote:'Cài gói router bằng Software Center. Thoát ứng dụng desktop cũ và tiến trình nền trước khi nâng cấp để tránh chiếm cổng hoặc tuyến mạng hai lần.',
 navLater:'Tính năng',
 laterLabel:'ĐÃ CÓ TRÊN ROUTER',
 laterTitle:'Nhóm, lối ra và lưu lượng, mỗi thứ một việc.',
 laterDescription:'Đây là những khả năng được thêm vào trang Merlin sau này. Ứng dụng desktop dùng cùng lõi. Phần dưới viết theo thao tác trên router.',
 laterKindGroup:'NHÓM',
 laterGroupTitle:'Nhóm node',
 laterGroupText:'Chọn thủ công chỉ dùng một node. Dự phòng thử theo thứ tự và, trong năm phút, bắt đầu từ thành viên vừa kết nối được. URL test giữ bắt tay nhanh hơn trong năm phút. Thứ tự không chọn người thắng.',
 laterKindExit:'LỐI RA',
 laterExitTitle:'Lối ra của quy tắc',
 laterExitText:'Node toàn cục đi theo node đang chọn ở đầu trang. Một node được ghim và không đổi theo. Nhóm node là lối ra riêng và không đổi node toàn cục. Chỉ chế độ Rule đọc các lối ra này.',
 laterKindTraffic:'LƯU LƯỢNG',
 laterTrafficTitle:'Thống kê lưu lượng',
 laterTrafficText:'Tổng quan tính byte từ khi tiến trình này khởi động và đặt lại khi khởi động lại. Trang subscription hiện ba tháng dương lịch gần nhất. Tổng của subscription là tổng các node hiện thuộc nó. Không hiện địa chỉ.',
 laterKindTrace:'KIỂM TRA',
 laterTraceTitle:'Kiểm tra đường đi',
 laterTraceText:'Nhập một trang trên phần quy tắc để xem quy tắc nào khớp và lối ra nào được dùng. Việc này chỉ giải thích quy tắc đã lưu, không mở kết nối proxy và không lưu truy vấn.',
 laterKindPreset:'MẪU',
 laterPresetTitle:'Mẫu quy tắc thường dùng',
 laterPresetText:'AI, video nước ngoài, web Telegram, công cụ lập trình, Apple, Microsoft và trang trong nước. Chúng được thêm ở trạng thái tắt. Danh sách là tên miền phổ biến, không phải mọi địa chỉ.',
 laterKindNet:'MẠNG',
 laterNetTitle:'IPv6 và công tắc',
 laterNetText:'Mặc định giữ tương thích IPv4 và không yêu cầu nhà mạng bật IPv6. Khởi động bật cùng lúc chặn DNS và proxy trong suốt. Khi đang chạy có thể tắt riêng từng cái. Bật proxy trong suốt cũng bật chặn DNS.',
 laterRouting:'Định tuyến có hiệu lực thế nào →',
 laterConfig:'Lưu lượng và DNS →',
 faqFiveQ:'Đã tạo nhóm, sao lưu lượng không đi qua nó?',
 faqFiveA:'Nhóm không tự chiếm lưu lượng. Để Tổng quan ở chế độ Rule, rồi chọn lối ra của quy tắc là Nhóm · tên. Quy tắc Direct và từ chối giữ nguyên. Global và Direct không đọc nhóm.',
 faqSixQ:'Vì sao tổng của subscription không khớp một node?',
 faqSixA:'Tổng cộng các node hiện thuộc subscription đó. Lưu lượng Direct và node không thuộc subscription chỉ nằm trong bảng node. Trang không hiện địa chỉ subscription.'
});
const elements = [...document.querySelectorAll('[data-t]')];
translations.zh = Object.fromEntries(elements.map(el => [el.dataset.t, el.innerHTML]));
Object.assign(translations.zh,{copied:'已复制',copyFailed:'请选中命令后手动复制'});
let locale = 'zh';
function setLanguage(value) {
 locale = Object.hasOwn(translations,value) ? value : 'zh';
 elements.forEach(el => { el.innerHTML = translations[locale][el.dataset.t] ?? translations.zh[el.dataset.t]; });
 document.documentElement.lang = {zh:'zh-CN',en:'en',vi:'vi'}[locale];
 document.title = {zh:'Sockrocket · 一台路由器，整屋一起走',en:'Sockrocket · One router, the whole home',vi:'Sockrocket · Một router, cả nhà cùng dùng'}[locale];
 document.querySelector('meta[name="description"]').content = document.querySelector('[data-t="heroDescription"]').textContent;
 document.getElementById('language').value = locale;
 try { localStorage.setItem('sockrocket-language',locale); } catch {}
}
let saved;
try { saved = localStorage.getItem('sockrocket-language'); } catch {}
setLanguage(saved || (navigator.language.startsWith('zh') ? 'zh' : navigator.language.startsWith('vi') ? 'vi' : 'en'));
document.getElementById('language').addEventListener('change',e => setLanguage(e.target.value));
const tabs=[...document.querySelectorAll('[data-screen]')];
function selectTab(tab) {
 tabs.forEach(el => { const active=el===tab;el.setAttribute('aria-selected',String(active));el.tabIndex=active?0:-1;el.classList.toggle('selected',active); });
 const src=`assets/${tab.dataset.screen}.svg`;
 document.getElementById('product-image').src=src;
 document.getElementById('product-image').alt=`${tab.dataset.screen} design illustration with synthetic data`;
 document.getElementById('product-zoom').dataset.zoom=src;
 document.getElementById('product-panel').setAttribute('aria-labelledby',tab.id);
}
tabs.forEach((tab,i) => {
 tab.addEventListener('click',()=>selectTab(tab));
 tab.addEventListener('keydown',e=>{
  let index;
  if(e.key==='ArrowRight'||e.key==='ArrowDown')index=(i+1)%tabs.length;
  else if(e.key==='ArrowLeft'||e.key==='ArrowUp')index=(i+tabs.length-1)%tabs.length;
  else if(e.key==='Home')index=0;
  else if(e.key==='End')index=tabs.length-1;
  else return;
  e.preventDefault();selectTab(tabs[index]);tabs[index].focus();
 });
});
const dialog=document.getElementById('image-dialog');
document.querySelectorAll('[data-zoom]').forEach(button=>button.addEventListener('click',()=>{
 document.getElementById('dialog-image').src=button.dataset.zoom;
 dialog.showModal();
}));
document.getElementById('close-dialog').addEventListener('click',()=>dialog.close());
dialog.addEventListener('click',e=>{if(e.target===dialog){const r=dialog.getBoundingClientRect();if(e.clientX<r.left||e.clientX>r.right||e.clientY<r.top||e.clientY>r.bottom)dialog.close();}});
document.getElementById('copy-command').addEventListener('click',async()=>{
 const button=document.getElementById('copy-command');
 try{
  await navigator.clipboard.writeText(document.getElementById('cli-command').textContent);
  button.textContent=translations[locale].copied;
  document.getElementById('announcement').textContent=translations[locale].copied;
  setTimeout(()=>{button.textContent=translations[locale].copy;},1800);
 }catch{document.getElementById('announcement').textContent=translations[locale].copyFailed;button.textContent=translations[locale].copyFailed;}
});
