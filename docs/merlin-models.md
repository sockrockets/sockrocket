# Merlin 路由器型号与离线安装包

在带软件中心的 KoolCenter 或 ASUSGO 固件上，按机身完整型号、硬件修订版和固件用户空间选择一个包。此表列出打包目标，真机验证取决于具体固件版本。各平台分别提供一个完整离线包。

| 路由器型号 | 固件用户空间 | 包内 .valid | 软件中心离线安装文件 |
|---|---|---|---|
| RT-AC68U、RT-AC68U V4、RT-AC66U B1、RT-AC1900、RT-AC1900P、RT-AC87U、Netgear R6300 V2、RT-AC88U、RT-AC3100、RT-AC3200、RT-AC5300 | ARMv7 软浮点 | `arm386` | `sockrocket-merlin-arm.tar.gz` |
| RT-BE92U、RT-BE82U、RT-BE58 Go、GS7 Pro、GS7 Pro Max、TUF-BE3600、TUF-BE3600 V2、TUF-AX5400 V2、ZenWiFi ET8、ZenWiFi XD6、ZenWiFi XT9、TUF-AX3000、TUF-AX3000 V2、TUF-AX5400、RT-AX58U、RT-AX58U V2、RAX70、RT-AX1800、RAX50、RT-AX82U、RT-AX82U V2、ZenWiFi XT8、ZenWiFi XT8 V2、ZenWiFi XD4、ZenWiFi XD4 Plus、RT-AX56U、RT-AX56U V2、RT-AX57、RT-AX55 | ARMv7 硬浮点 | `hnd` | `sockrocket-merlin-hnd.tar.gz` |
| RT-AC86U、GT-AC2900、GT-AC5300、RT-AX88U、RT-AX88U PRO、RAX80、GT-AX11000、GT-AX11000 PRO、RT-AX92U、RT-AX68U、RT-AX86U、RT-AX86U PRO、RT-AX86S、GT-AXE11000、GT-AXE16000、GT-AX6000、ZenWiFi Pro XT12、RT-BE88U、RT-BE7200、RT-BE86U、RT-BE96U、GT-BE96、GT-BE98、GT-BE98 Pro、GT-BE19000AI | AArch64 | `hnd_v8` | `sockrocket-merlin-hnd_v8.tar.gz` |
| RT-AX89X | ARMv7 硬浮点 | `qca` | `sockrocket-merlin-qca.tar.gz` |
| GS7、GS7 Air、GS-BE7200X、TX-AX6000、ZenWiFi BT8P、RT-AX57 Go、TUF-AX4200Q、TUF-AX4200、TUF-AX6000、RT-AX59U | AArch64 | `mtk` | `sockrocket-merlin-mtk.tar.gz` |
| ZenWiFi BD4 | ARMv7 硬浮点 | `ipq32` | `sockrocket-merlin-ipq32.tar.gz` |
| TUF-BE6500 | AArch64 | `ipq64` | `sockrocket-merlin-ipq64.tar.gz` |

RT-AC88U 的 Koolshare 384/386 软件中心接受 `arm384` 或 `arm386` 标记。原始 v0.0.19 发布包的标记是 `arm`，安装时请选择已修正标记的构建。RT-AX88U 和 RT-AX88U PRO 对应 64 位博通包。ZenWiFi BD4 和 TUF-BE6500 均采用 IPQ5322，表中分别对应其 32 位和 64 位固件。

参考：[传统 ARM 软件中心的校验规则](https://github.com/koolshare/armsoft/blob/master/softcenter/softcenter/scripts/ks_tar_install.sh)、[其他平台的软件中心规则](https://github.com/koolshare/rogsoft/blob/master/softcenter/softcenter/scripts/ks_tar_install.sh)、[机型和固件架构表](https://github.com/hq450/fancyss/blob/3.0/README.md)。
