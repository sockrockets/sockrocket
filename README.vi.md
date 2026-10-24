# Sockrocket

<p align="center">
  <img src="crates/sockrocket-gui/assets/logo-banner.svg" alt="Sockrocket" width="320" />
</p>
<p align="center"><strong>Một lõi proxy Rust cho máy tính và router.</strong></p>
<p align="center">Ứng dụng desktop native · CLI · Plugin AsusWRT-Merlin</p>
<p align="center"><a href="README.md">English</a> · <strong>Tiếng Việt</strong> · <a href="README.zh.md">中文</a></p>
<p align="center">
  <a href="https://sockrockets.github.io/sockrocket/">Website</a> · <a href="https://t.me/sockrocket">Telegram · @sockrocket</a> ·
  <a href="https://github.com/sockrockets/sockrocket/releases/latest">Tải xuống</a> ·
  <a href="docs/getting-started.md">Bắt đầu</a> ·
  <a href="docs/routing.md">Định tuyến</a> ·
  <a href="docs/merlin.md">Hướng dẫn Merlin</a> ·
  <a href="https://github.com/sockrockets/sockrocket/issues">Báo lỗi</a>
</p>

Sockrocket quản lý node, subscription, quy tắc định tuyến và chế độ chuyển lưu lượng trong cùng một ứng dụng. Giao diện desktop theo phong cách nhóm cài đặt của macOS; CLI và plugin Merlin dùng chung lõi giao thức Rust.

> **Về hình ảnh:** hình desktop theo định hướng Figma và dùng dữ liệu giả; giao diện phát hành có thể khác. Hình Merlin được chụp từ giao diện quản lý thực tế sau khi ẩn node, địa chỉ, SSID và thông tin xác thực trước khi xuất.

![Thiết kế tổng quan: trạng thái kết nối, lưu lượng và proxy hệ thống](docs/images/readme/overview.png)

## Tính năng

| Khả năng | Mục đích |
| --- | --- |
| Giao thức native | Shadowsocks, VMess, VLESS, Trojan, TUIC, Hysteria2, AnyTLS và WireGuard |
| Node và subscription | Nhập liên kết node hoặc subscription Clash / V2Ray / sing-box; cập nhật, tìm kiếm, lọc trùng và đo độ trễ |
| Định tuyến | Rule, Global và Direct. Lối ra là node toàn cục, một node, hoặc một nhóm node. Xem [Định tuyến](docs/routing.md) |
| Chuyển lưu lượng | Proxy hệ thống cho ứng dụng tương thích; TUN để xử lý nhiều ứng dụng hơn |
| Kết nối và lưu lượng | Tổng quan tính byte từ khi tiến trình này khởi động. Trang subscription hiện ba tháng dương lịch gần nhất theo node và theo subscription |
| Dùng chung lõi | GUI desktop, CLI và Merlin WebUI |
| Ngôn ngữ | English, Tiếng Việt và 中文 |

Các tổ hợp giao thức có giới hạn: ShadowTLS không cung cấp UDP; UDP native của VMess và WireGuard có giới hạn kích thước mỗi gói. VLESS Vision hỗ trợ TLS 1.3/REALITY qua TCP và XUDP; flow tiêu chuẩn từ chối UDP/443. gRPC/XHTTP chưa được triển khai và sẽ bị từ chối rõ ràng. Xem [hướng dẫn giao thức](docs/protocols.md). Sockrocket không cung cấp node hoặc dịch vụ subscription.

## Giao thức mới: AnyTLS và WireGuard

> **Phạm vi phiên bản:** các tính năng này mô tả mã nguồn hiện tại. Gói trên GitHub Releases tuân theo ghi chú của từng phiên bản.

| Giao thức | Khả năng | Giới hạn hiện tại |
| --- | --- | --- |
| AnyTLS v2 | TLS native, tái sử dụng phiên, TCP và UDP-over-TCP v2; nhập liên kết, Clash và sing-box | Mặc định xác minh chứng chỉ; UDP được truyền qua TCP |
| WireGuard | TCP / UDP trong tiến trình, khóa chia sẻ trước và DNS trong đường hầm; nhập cấu hình một peer, Clash và sing-box | Một peer mỗi node; UDP phải nằm trong MTU; chưa hỗ trợ phân mảnh IP hoặc script cấu hình |

Desktop, CLI và Merlin dùng chung cách triển khai và cơ chế lưu cấu hình trên thiết bị. Đường truyền WireGuard không tạo thêm giao diện mạng hay thay đổi tuyến mạng và DNS của hệ thống; chế độ TUN của ứng dụng được điều khiển riêng. Cả hai giao thức đã vượt qua kiểm tra tương thích với máy chủ độc lập.

Xem [hướng dẫn giao thức](docs/protocols.md) để biết định dạng, giới hạn và phần Vision, gRPC/XHTTP chưa triển khai.

## Giao diện và thao tác

### Quản lý nguồn và chọn node riêng biệt

Subscription quản lý nguồn node; lựa chọn node xác định đường ra hiện tại. Phép đo độ trễ đi qua proxy. Đơn vị mili giây thể hiện độ trễ phản hồi lúc đo, không phải băng thông tải xuống.

![Thiết kế danh sách node với dữ liệu và độ trễ giả](docs/images/readme/nodes.png)

### Xem và chỉnh sửa quy tắc

Kiểm tra điều kiện khớp, đích và trạng thái bật. Thứ tự và độ ưu tiên ảnh hưởng kết quả. Nhóm quy tắc là thư mục quy tắc. Nhóm node là lối ra. Chỉ chế độ Rule đọc chúng.

![Thiết kế danh sách quy tắc và thao tác chỉnh sửa](docs/images/readme/rules.png)

### Chỉnh sửa nhiều quy tắc bằng văn bản

Dán hàng loạt và sửa theo nhóm giúp chuyển quy tắc có sẵn. Kiểm tra kết quả phân tích, đích và cách xử lý xung đột trước khi lưu. Domain `example.com` được dành cho tài liệu.

![Thiết kế dán quy tắc, kiểm tra và xác nhận](docs/images/readme/rules-bulk.png)

## Lưu lượng của một quy tắc đi đâu

Chế độ Global dùng node đang chọn ở đầu trang. Direct không dùng proxy. Chỉ chế độ Rule đọc quy tắc.

| Lối ra | Đi theo |
| --- | --- |
| Node toàn cục | Node đang chọn ở đầu trang, kể cả khi kiểm tra sức khỏe tự đổi |
| Một node | Chỉ node đó. Đổi lựa chọn ở đầu trang không làm quy tắc này đổi theo |
| Nhóm node | Thành viên và chiến lược của nhóm. Đổi thành viên không đổi node toàn cục |

Nhóm thủ công dùng một node. Dự phòng thử theo thứ tự và, trong năm phút, bắt đầu lại từ lần kết nối thành công gần nhất; UDP luôn đi qua thành viên đầu tiên. URL test giữ bắt tay nhanh nhất trong năm phút, thứ tự danh sách không chọn người thắng. Nhóm chưa có tác dụng cho đến khi lối ra của một quy tắc là nhóm đó. Các bước: [Định tuyến](docs/routing.md).

## Trên trang Merlin

Ứng dụng desktop dùng cùng lõi này. Các thao tác dưới đây nằm trong WebUI của router.

| Trang | Việc có thể làm |
| --- | --- |
| Tổng quan | Khởi động bật cùng lúc chặn DNS và proxy trong suốt. Khi đang chạy có thể tắt riêng từng cái. Lưu lượng ở đây tính từ khi tiến trình này khởi động |
| Node | Dùng một node hoặc tạo nhóm node. Node được ghim mà không còn thì thất bại, không chuyển sang node toàn cục. Tự đổi khi kiểm tra sức khỏe chỉ di chuyển node toàn cục |
| Subscription | Cập nhật subscription. Ba tháng gần nhất gồm node thủ công và Direct. Tổng của subscription là tổng các node hiện thuộc nó. Không hiện địa chỉ |
| Quy tắc định tuyến | Chọn lối ra cho một thư mục quy tắc, kiểm tra một trang sẽ khớp đâu mà không mở kết nối, hoặc thêm mẫu. Mẫu được thêm ở trạng thái tắt: AI, video nước ngoài, web Telegram, công cụ lập trình, Apple, Microsoft và trang trong nước. Đó là tên miền phổ biến, không phải mọi địa chỉ |
| Mặc định theo thiết bị | Lối ra cuối cho một thiết bị hoặc một dải. Thiết bị khác vẫn dùng quy tắc chung. Hãy dùng địa chỉ DHCP cố định. Kế thừa toàn cục xóa quy tắc cuối của nguồn đó và giữ ngoại lệ theo đích |
| Mạng và DNS | Mặc định tương thích IPv4, không yêu cầu nhà mạng bật IPv6. Dual-stack chỉ khi firmware đã có địa chỉ IPv6 và tiền tố LAN. Lưu áp lại tường lửa tại chỗ, không khởi động lại proxy đang chạy |

## Tải xuống và kết nối

Mỗi tên tệp dưới đây là đúng tệp đó trong [bản phát hành mới nhất](https://github.com/sockrockets/sockrocket/releases/latest). [Bảng tải trên website](https://sockrockets.github.io/sockrocket/#download) dùng cùng tên và trỏ tới bản hiện tại khi mở trang. Gói router được liệt kê ở đó theo nền tảng firmware.

| Hệ thống | Gói desktop |
| --- | --- |
| macOS · Apple Silicon | [sockrocket-macos-aarch64.dmg](https://github.com/sockrockets/sockrocket/releases/latest/download/sockrocket-macos-aarch64.dmg) |
| macOS · Intel | [sockrocket-macos-x86_64.dmg](https://github.com/sockrockets/sockrocket/releases/latest/download/sockrocket-macos-x86_64.dmg) |
| Windows · x86_64 | [sockrocket-windows-x86_64.zip](https://github.com/sockrockets/sockrocket/releases/latest/download/sockrocket-windows-x86_64.zip) |
| Linux · x86_64 | [sockrocket-linux-x86_64.tar.gz](https://github.com/sockrockets/sockrocket/releases/latest/download/sockrocket-linux-x86_64.tar.gz) |
| Linux · ARM64 | [sockrocket-linux-aarch64.tar.gz](https://github.com/sockrockets/sockrocket/releases/latest/download/sockrocket-linux-aarch64.tar.gz) |

1. Cài đặt và mở ứng dụng. Trên macOS, kéo ứng dụng vào Applications trước; trên Windows, giải nén ZIP; trên Linux, làm theo hướng dẫn trong gói.
2. Thêm subscription của bạn trong **Subscriptions**, hoặc nhập liên kết ở **Nodes**.
3. Cập nhật subscription và chọn node; đo độ trễ nếu cần.
4. Nhấn **Connect** ở **Overview**, chờ kiểm tra kết nối rồi kiểm tra proxy hệ thống.
5. Chỉ bật **TUN** khi cần và hoàn tất yêu cầu quyền quản trị của hệ điều hành.

Trước khi nâng cấp, thoát bản cũ và tiến trình nền chạy với quyền quản trị. Hai phiên bản chạy cùng lúc có thể tranh chấp cổng và tuyến TUN.

## AsusWRT-Merlin

Plugin router cung cấp WebUI, proxy trong suốt, chuyển DNS và kiểm tra dịch vụ cho các thiết bị LAN.

![Giao diện quản lý Merlin thực tế, đã ẩn node, địa chỉ, SSID và thông tin xác thực](docs/images/readme/merlin.jpg)

*Ảnh đã ẩn thông tin. Số liệu là trạng thái lúc chụp, không phải trạng thái trực tiếp.*

1. Nếu menu chưa có Software Center, tải bản Merlin hoặc bản mod chính hãng đúng model từ [fw.koolcenter.com](https://fw.koolcenter.com/) rồi flash. Merlin gốc không có Software Center.
2. Chọn `sockrocket-merlin-<platform>.tar.gz` đúng nền tảng firmware và kiến trúc CPU; không chỉ đoán theo tên model.
3. Làm theo [hướng dẫn Merlin](docs/merlin.md) để cấu hình JFFS / custom scripts và cài gói.
4. Cấu hình subscription trên router; kiểm tra node trước khi bật chuyển DNS hoặc proxy trong suốt. Để một phần lưu lượng đi qua nhóm node, giữ chế độ Rule và đặt lối ra của quy tắc thành nhóm đó.
5. Sao lưu cấu hình trước khi cập nhật và kiểm tra thao tác dừng / khôi phục mạng trước khi sử dụng lâu dài.

```sh
/jffs/addons/sockrocket/scripts/sockrocket.sh status
/jffs/addons/sockrocket/scripts/sockrocket.sh stop
/jffs/addons/sockrocket/scripts/sockrocket.sh start
```

Chi tiết nền tảng, dịch vụ và khôi phục: [Merlin](docs/merlin.md) · [Tài liệu kỹ thuật](docs/merlin-reference.md).

## Proxy hệ thống hay TUN?

| Chế độ | Trường hợp sử dụng | Lưu ý |
| --- | --- | --- |
| Proxy hệ thống | Trình duyệt và ứng dụng tuân theo cài đặt proxy của OS | Một số ứng dụng bỏ qua cài đặt này |
| TUN | Chuyển lưu lượng của nhiều ứng dụng hơn | Cần quyền quản trị và hỗ trợ TUN; tắt TUN / VPN khác trước |
| CLI listener | Công cụ terminal hoặc ứng dụng cấu hình thủ công | Mặc định chỉ lắng nghe trên máy; cấu hình ứng dụng gọi proxy |

Cổng local mặc định: SOCKS5 `127.0.0.1:1080`, HTTP `127.0.0.1:1087`. Đây là listener loopback, không phải node từ xa.

## Dòng lệnh

CLI có trên cùng trang phát hành, tên file bắt đầu bằng `sockrocket-cli-`.

```sh
sockrocket-cli --init config.yaml
# Chỉnh cấu hình local với subscription hoặc node của bạn.
sockrocket-cli config.yaml
```

Dừng bằng `Ctrl+C`. Xem [tài liệu cấu hình](docs/configuration.md) để biết các trường và ví dụ an toàn. Không commit cấu hình có thông tin xác thực thật.

## Xử lý sự cố

| Hiện tượng | Kiểm tra đầu tiên |
| --- | --- |
| Connect không khởi động được | Tiến trình nền bản cũ, cổng bị chiếm và lỗi trong log |
| Không bật được TUN | Quyền quản trị, driver và TUN khác đang giữ tuyến mặc định |
| Đã kết nối nhưng không mở được web | Kiểm tra riêng node, proxy hệ thống / TUN, DNS và quy tắc |
| Subscription lỗi hoặc không có node | Kiểm tra URL, định dạng, phản hồi máy chủ và kết quả phân tích trên máy |
| Chỉ một số trang bị lỗi | Quy tắc khớp, đường ra Direct, DNS và nhiều client cùng chuyển lưu lượng |
| Đã tạo nhóm nhưng lưu lượng không đi qua | Tổng quan phải ở chế độ Rule, và lối ra của một quy tắc phải là Nhóm · tên. Global và Direct bỏ qua nhóm |
| Khởi động lại thì lưu lượng như bị xóa | Bộ đếm trên Tổng quan bắt đầu lại cùng tiến trình. Ba tháng nằm ở trang Subscription |

## Cộng đồng & liên hệ

| | |
| --- | --- |
| [Telegram · @sockrocket](https://t.me/sockrocket) | Thông báo phát hành và cập nhật |
| [GitHub Discussions](https://github.com/sockrockets/sockrocket/discussions) | Hỏi đáp và ý tưởng |
| [GitHub Issues](https://github.com/sockrockets/sockrocket/issues) | Báo lỗi kèm bước tái hiện |
| [Website](https://sockrockets.github.io/sockrocket/) | Giới thiệu, tải xuống và tài liệu |

## Quyền riêng tư và phản hồi

Hình thiết kế desktop dùng dữ liệu giả; ảnh Merlin được ẩn thông tin trước khi xuất. Trước khi chia sẻ Issue, log hoặc ảnh, hãy xóa URL subscription đầy đủ và tham số truy vấn, địa chỉ máy chủ, mật khẩu, UUID, token, mã QR, đường dẫn cá nhân và thông tin tài khoản. Mã QR và liên kết chia sẻ có thể chứa toàn bộ thông tin xác thực.

- [Issues](https://github.com/sockrockets/sockrocket/issues): báo lỗi.
- [Discussions](https://github.com/sockrockets/sockrocket/discussions): hỏi đáp.
- [Mục lục tài liệu](docs/README.md): định tuyến, cấu hình, giao thức, kiến trúc và phát triển.

## Giấy phép

[Apache License 2.0](LICENSE). Xem [ai.txt](ai.txt), [robots.txt](robots.txt) và [tuyên bố TDM](.well-known/tdmrep.json) về việc sử dụng cho huấn luyện AI / ML.
