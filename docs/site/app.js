'use strict';
const translations = {
en: {
skip:'Skip to content',navExperience:'Experience',navDownload:'Download',navCommunity:'Community',heroLabel:'Native client · Open-source Rust engine',heroTitle:'From your desktop.<br>To your router.',heroDescription:'Nodes, subscriptions, rules and connection status, clearly in one place. One proxy engine, across your devices.',heroDownload:'Download Sockrocket',heroGuide:'Read the guide',heroPlatforms:'macOS · Windows · Linux · AsusWRT-Merlin',heroWindow:'Familiar layout. Clear controls.',imageNote:'Design illustration · Synthetic data; released interfaces may differ',floatTitle:'Connection, with a way back',floatText:'Saved configuration · Proxy and route recovery',protocolLabel:'One engine. The protocols you know.',experienceLabel:'LESS SEARCHING. MORE CONTROL.',experienceTitle:'A place for every setting.',experienceDescription:'A grouped layout inspired by macOS System Settings, with straightforward everyday controls and details within reach.',tabNodes:'Nodes & subscriptions',tabNodesSub:'Clear sources. Simple selection.',tabRules:'Rules & routing',tabRulesSub:'Inspect, edit and choose an exit.',tabBulk:'Bulk rule editing',tabBulkSub:'Paste, review, then save.',workspaceNote:'Rule, Global or Direct.<br>Choose for your use case.',capOneTitle:'Choose how to capture traffic',capOneText:'System proxy for applications that follow proxy settings; TUN when you need broader traffic capture.',capTwoTitle:'See the connection details',capTwoText:'Inspect the selected exit, connections, traffic and logs instead of relying on a Connected label alone.',capThreeTitle:'Keep your configuration locally',capThreeText:'Subscription URLs, nodes and selection are saved. Sockrocket does not provide nodes or subscriptions.',merlinLabel:'SOCKROCKET FOR MERLIN',merlinTitle:'Your router.<br>More connected devices.',merlinDescription:'Manage nodes, subscriptions and routing in the router WebUI. Transparent proxying and DNS interception bring the same engine to LAN devices.',merlinGuide:'Merlin installation guide',merlinNote:'Packages must match firmware platform and CPU architecture. Check the guide for compatibility.',routerOne:'Nodes & subscriptions',routerTwo:'Transparent routing',routerThree:'Name resolution',routerBottom:'One engine. Multiple devices.',networkNote:'Architecture illustration, not live network status.',downloadLabel:'CHOOSE YOUR STARTING POINT',downloadTitle:'Start on the device you use.',downloadDescription:'All release files are on GitHub Releases. Choose your OS and architecture; CLI and Merlin packages are on the same page.',downloadMac:'Download .dmg ↗',downloadWindows:'Download .zip ↗',downloadLinux:'Download .tar.gz ↗',downloadRouterSub:'Match your firmware platform',downloadRouter:'Choose a router package ↗',downloadNote:'Quit the old version and its background instance before upgrading to avoid competing ports or routes.',releaseHistory:'All releases & changelog ↗',startLabel:'GETTING STARTED',startTitle:'Three steps to your connection.',startGuide:'Complete getting-started guide ↗',stepOneTitle:'Add your subscription or nodes',stepOneText:'Import node links or Clash, V2Ray and sing-box subscriptions.',stepTwoTitle:'Select a node and review routing',stepTwoText:'Test latency, then choose Rule, Global or Direct mode for your needs.',stepThreeTitle:'Connect and verify access',stepThreeText:'Verify the system proxy first. Enable TUN and authorize it when broader capture is needed.',cliTitle:'Or start from the terminal.',cliGuide:'Configuration reference ↗',copy:'Copy commands',copied:'Copied',copyFailed:'Select and copy the commands manually',cliNote:'Before connecting, add your own nodes or subscriptions to the local configuration file.',faqTitle:'A few things to know first.',faqOneQ:'Does Sockrocket provide proxy nodes?',faqOneA:'No. Sockrocket is a proxy client and engine. You need your own working nodes or subscription.',faqTwoQ:'System proxy or TUN?',faqTwoA:'System proxy covers applications that follow OS proxy settings. TUN captures more traffic and requires administrator privileges and a working driver. Avoid running multiple TUNs or VPNs together.',faqThreeQ:'Which Merlin package should I choose?',faqThreeA:'Match the firmware platform tag and CPU architecture, rather than guessing from the router name. Read the Merlin guide, back up configuration and check JFFS space before upgrading.',faqFourQ:'What should I include in a bug report?',faqFourA:'Include software and OS/firmware versions, reproduction steps and sanitized logs. Remove full subscription URLs, node addresses, passwords, UUIDs, tokens, QR codes and personal paths.',communityLabel:'BUILD BETTER CONNECTIONS, TOGETHER',communityTitle:'Updates. Questions. Feedback.<br>Find us here.',telegramDescription:'Release announcements & updates · @sockrocket',discussionsDescription:'Usage questions, ideas and conversations',issuesDescription:'Bug reports with sanitized reproduction steps',footer:'Open-source Rust proxy client · Apache-2.0',docs:'Docs'
},
vi: {
skip:'Đến nội dung',navExperience:'Trải nghiệm',navDownload:'Tải xuống',navCommunity:'Cộng đồng',heroLabel:'Ứng dụng native · Lõi Rust mã nguồn mở',heroTitle:'Từ máy tính.<br>Đến router.',heroDescription:'Node, subscription, quy tắc và trạng thái kết nối, rõ ràng trong cùng một nơi. Một lõi proxy cho nhiều thiết bị.',heroDownload:'Tải Sockrocket',heroGuide:'Đọc hướng dẫn',heroPlatforms:'macOS · Windows · Linux · AsusWRT-Merlin',heroWindow:'Bố cục quen thuộc. Điều khiển rõ ràng.',imageNote:'Minh họa thiết kế · Dữ liệu giả; giao diện phát hành có thể khác',floatTitle:'Kết nối và khôi phục',floatText:'Lưu cấu hình · Khôi phục proxy và tuyến mạng',protocolLabel:'Một lõi. Những giao thức quen thuộc.',experienceLabel:'DỄ TÌM. DỄ ĐIỀU KHIỂN.',experienceTitle:'Mỗi cài đặt, một vị trí rõ ràng.',experienceDescription:'Bố cục nhóm theo phong cách macOS System Settings, đơn giản cho thao tác thường ngày và dễ tìm chi tiết.',tabNodes:'Node & subscription',tabNodesSub:'Nguồn rõ ràng, dễ lựa chọn.',tabRules:'Quy tắc & định tuyến',tabRulesSub:'Xem, chỉnh sửa, chọn lối ra.',tabBulk:'Sửa quy tắc hàng loạt',tabBulkSub:'Dán, kiểm tra, rồi lưu.',workspaceNote:'Rule, Global hoặc Direct.<br>Chọn theo nhu cầu sử dụng.',capOneTitle:'Chọn cách chuyển lưu lượng',capOneText:'Proxy hệ thống cho ứng dụng tuân theo cài đặt proxy; TUN khi cần chuyển nhiều lưu lượng hơn.',capTwoTitle:'Xem chi tiết kết nối',capTwoText:'Kiểm tra lối ra, kết nối, lưu lượng và nhật ký thay vì chỉ dựa vào nhãn Đã kết nối.',capThreeTitle:'Lưu cấu hình trên thiết bị',capThreeText:'URL subscription, node và lựa chọn được lưu. Sockrocket không cung cấp node hay subscription.',merlinLabel:'SOCKROCKET FOR MERLIN',merlinTitle:'Một router.<br>Kết nối nhiều thiết bị.',merlinDescription:'Quản lý node, subscription và định tuyến trong WebUI của router. Proxy trong suốt và DNS đưa cùng lõi đến các thiết bị LAN.',merlinGuide:'Hướng dẫn cài Merlin',merlinNote:'Gói cài đặt phải khớp nền tảng firmware và kiến trúc CPU. Xem hướng dẫn để kiểm tra tương thích.',routerOne:'Node & subscription',routerTwo:'Định tuyến trong suốt',routerThree:'Phân giải tên miền',routerBottom:'Một lõi. Nhiều thiết bị.',networkNote:'Minh họa kiến trúc, không phải trạng thái mạng trực tiếp.',downloadLabel:'CHỌN ĐIỂM BẮT ĐẦU',downloadTitle:'Bắt đầu trên thiết bị của bạn.',downloadDescription:'Tất cả tệp phát hành nằm trên GitHub Releases. Chọn hệ điều hành và kiến trúc; CLI và gói Merlin cũng ở cùng trang.',downloadMac:'Tải .dmg ↗',downloadWindows:'Tải .zip ↗',downloadLinux:'Tải .tar.gz ↗',downloadRouterSub:'Chọn đúng nền tảng firmware',downloadRouter:'Chọn gói cho router ↗',downloadNote:'Thoát phiên bản cũ và tiến trình nền trước khi nâng cấp để tránh xung đột cổng hoặc tuyến mạng.',releaseHistory:'Các phiên bản & thay đổi ↗',startLabel:'BẮT ĐẦU',startTitle:'Ba bước để kết nối.',startGuide:'Hướng dẫn sử dụng đầy đủ ↗',stepOneTitle:'Thêm subscription hoặc node của bạn',stepOneText:'Nhập liên kết node hoặc subscription Clash, V2Ray và sing-box.',stepTwoTitle:'Chọn node và kiểm tra định tuyến',stepTwoText:'Đo độ trễ, rồi chọn chế độ Rule, Global hoặc Direct theo nhu cầu.',stepThreeTitle:'Kết nối và kiểm tra truy cập',stepThreeText:'Kiểm tra proxy hệ thống trước. Bật TUN và cấp quyền khi cần chuyển thêm lưu lượng.',cliTitle:'Hoặc bắt đầu từ terminal.',cliGuide:'Tham khảo cấu hình ↗',copy:'Sao chép lệnh',copied:'Đã sao chép',copyFailed:'Vui lòng chọn và sao chép lệnh thủ công',cliNote:'Trước khi kết nối, thêm node hoặc subscription của bạn vào tệp cấu hình trên máy.',faqTitle:'Một vài điều cần biết trước.',faqOneQ:'Sockrocket có cung cấp node không?',faqOneA:'Không. Sockrocket là ứng dụng và lõi proxy. Bạn cần có node hoặc subscription hoạt động của riêng mình.',faqTwoQ:'Proxy hệ thống hay TUN?',faqTwoA:'Proxy hệ thống dành cho ứng dụng tuân theo cài đặt proxy của OS. TUN chuyển nhiều lưu lượng hơn và cần quyền quản trị cùng driver phù hợp. Tránh chạy nhiều TUN hoặc VPN đồng thời.',faqThreeQ:'Nên chọn gói Merlin nào?',faqThreeA:'Chọn theo nhãn nền tảng firmware và kiến trúc CPU, không chỉ theo tên router. Đọc hướng dẫn Merlin, sao lưu cấu hình và kiểm tra dung lượng JFFS trước khi nâng cấp.',faqFourQ:'Cần gửi gì khi báo lỗi?',faqFourA:'Gửi phiên bản phần mềm, OS/firmware, bước tái hiện và nhật ký đã ẩn thông tin. Xóa URL subscription đầy đủ, địa chỉ node, mật khẩu, UUID, token, mã QR và đường dẫn cá nhân.',communityLabel:'CÙNG NHAU CẢI THIỆN KẾT NỐI',communityTitle:'Cập nhật. Hỏi đáp. Góp ý.<br>Liên hệ tại đây.',telegramDescription:'Thông báo phát hành & cập nhật · @sockrocket',discussionsDescription:'Hỏi đáp, ý tưởng và trao đổi sử dụng',issuesDescription:'Báo lỗi với bước tái hiện đã ẩn thông tin',footer:'Ứng dụng proxy Rust mã nguồn mở · Apache-2.0',docs:'Tài liệu'
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
 protocolGuide:'Full protocol and transport guide ↗'
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
 protocolGuide:'Hướng dẫn giao thức và transport đầy đủ ↗'
});
const elements = [...document.querySelectorAll('[data-t]')];
translations.zh = Object.fromEntries(elements.map(el => [el.dataset.t, el.innerHTML]));
Object.assign(translations.zh,{copied:'已复制',copyFailed:'请选中命令后手动复制'});
let locale = 'zh';
function setLanguage(value) {
 locale = Object.hasOwn(translations,value) ? value : 'zh';
 elements.forEach(el => { el.innerHTML = translations[locale][el.dataset.t] ?? translations.zh[el.dataset.t]; });
 document.documentElement.lang = {zh:'zh-CN',en:'en',vi:'vi'}[locale];
 document.title = {zh:'Sockrocket · 从桌面，到路由器',en:'Sockrocket · From desktop to router',vi:'Sockrocket · Từ máy tính đến router'}[locale];
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
