"""Generate editable SVG screens without third-party dependencies."""
from pathlib import Path
from html import escape
import json
import re

ROOT = Path(__file__).parent
ICONS = {p.stem: p.read_text() for p in (ROOT.parents[2] / 'crates/sockrocket-gui/assets/icons').glob('*.svg')}
NAV = [('概览', 'bolt'), ('节点', 'nav-nodes'), ('订阅', 'globe'), ('策略组', 'nav-groups'), ('分流规则', 'nav-rules'), ('连接', 'nav-connections'), ('日志', 'nav-logs'), ('网络设置', 'settings-2'), ('通用', 'nav-settings')]


class Screen:
    def __init__(self, name, index, subtitle):
        self.parts = []
        self.name = name
        self.y = 115
        self.rect(0, 0, 1060, 800, '#f5f5f7', 13)
        self.rect(0, 0, 246, 800, '#eaeaf0', 13)
        self.rect(232, 0, 14, 800, '#eaeaf0')
        for x, color in [(24, '#ff5f57'), (44, '#febc2e'), (64, '#28c840')]:
            self.parts.append(f'<circle cx="{x}" cy="25" r="6" fill="{color}"/>')
        self.icon('sockrocket', 23, 53, '#007aff', 28)
        self.text(62, 73, 'Sockrocket', 15, weight=600)
        self.rect(18, 99, 210, 29, '#ffffff80', 7)
        self.icon('search', 27, 106, '#6e6e73', 15)
        self.text(47, 119, '搜索设置', 12, '#6e6e73')
        y = 148
        for i, (label, ic) in enumerate(NAV):
            if i in (3, 7):
                self.parts.append(f'<path d="M28 {y+2}H218" stroke="#d9d9df"/>')
                y += 18
            if i == index:
                self.rect(15, y, 216, 36, '#007aff', 7)
            self.icon(ic, 26, y+9, '#ffffff' if i == index else '#007aff')
            self.text(57, y+23, label, 13, '#ffffff' if i == index else '#1d1d1f')
            y += 40
        self.parts.append('<circle cx="29" cy="768" r="4" fill="#248a3d"/>')
        self.text(42, 772, '已连接 · 香港 01', 11, '#6e6e73')
        self.text(286, 48, name, 22, weight=650)
        self.text(286, 79, subtitle, 12, '#6e6e73')

    def rect(self, x, y, w, h, color, radius=0):
        self.parts.append(f'<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="{radius}" fill="{color}"/>')

    def text(self, x, y, value, size=13, color='#1d1d1f', weight=400):
        self.parts.append(f'<text x="{x}" y="{y}" font-family="SF Pro Text, -apple-system, PingFang SC, Noto Sans SC, sans-serif" font-size="{size}" font-weight="{weight}" fill="{color}">{escape(str(value))}</text>')

    def icon(self, name, x, y, color, size=18):
        svg = ICONS.get(name, '')
        inner = re.sub(r'^<svg[^>]*>|</svg>\s*$', '', svg.strip()).replace('currentColor', color)
        self.parts.append(f'<g transform="translate({x} {y}) scale({size/24})" fill="none" stroke="{color}" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">{inner}</g>')

    def button(self, label, x, y, primary=False):
        w = len(label)*13+26
        self.rect(x, y, w, 30, '#007aff' if primary else '#ffffff', 7)
        self.text(x+13, y+20, label, color='#ffffff' if primary else '#1d1d1f')
        return w+9

    def buttons(self, labels):
        x=286
        for label in labels:
            x += self.button(label, x, self.y)
        self.y += 48

    def toggle(self, x, y, on):
        self.rect(x, y, 33, 20, '#007aff' if on else '#d2d2d7', 10)
        self.parts.append(f'<circle cx="{x+(23 if on else 10)}" cy="{y+10}" r="8" fill="#ffffff"/>')

    def group(self, title, rows):
        self.text(286, self.y+12, title, 13, weight=600)
        self.y += 25
        self.rect(286, self.y, 734, len(rows)*47, '#ffffff', 10)
        for i, (label, value) in enumerate(rows):
            y=self.y+i*47
            if i:
                self.parts.append(f'<path d="M286 {y}H1020" stroke="#dedee3"/>')
            self.text(302, y+30, label)
            if isinstance(value, bool):
                self.toggle(969, y+15, value)
            else:
                self.parts.append(f'<text x="1002" y="{y+30}" text-anchor="end" font-family="SF Pro Text, PingFang SC, Noto Sans SC, sans-serif" font-size="13" fill="#6e6e73">{escape(str(value))}</text>')
        self.y += len(rows)*47+14

    def note(self, value, color='#6e6e73'):
        self.text(288, self.y+10, value, 11, color)
        self.y += 30

    def hero(self, status, connected=True):
        self.rect(286,self.y,734,140,'#ffffff',10)
        self.icon('circle-check' if connected else 'circle',308,self.y+22,'#248a3d' if connected else '#6e6e73',30)
        self.text(347,self.y+46,status,27,'#248a3d' if connected else '#6e6e73',600)
        self.button('断开连接' if connected else '连接',908,self.y+24,not connected)
        self.text(309,self.y+85,'香港 01 · Shadowsocks',13,weight=600)
        self.text(309,self.y+111,'规则模式 · '+('已连接 00:24:18' if connected else '节点已就绪'),11,'#6e6e73')
        self.y += 160

    def save(self, filename):
        if self.y > 790:
            raise ValueError(f'{filename}: content exceeds screen ({self.y})')
        data='<svg xmlns="http://www.w3.org/2000/svg" width="1060" height="800" viewBox="0 0 1060 800"><title>'+escape(self.name)+'</title>'+''.join(self.parts)+'</svg>'
        (ROOT / 'figma-svg' / filename).write_text(data)
        return ''.join(self.parts)


def build():
    (ROOT/'figma-svg').mkdir(exist_ok=True)
    screens=[]
    s=Screen('概览',0,'网络状态、当前出口与常用操作。')
    s.rect(286,110,734,173,'#ffffff',10)
    s.rect(308,132,42,42,'#248a3d0b',12)
    s.icon('circle-check',316,140,'#248a3d',26)
    s.text(363,153,'代理服务运行中',23,'#248a3d',600)
    s.text(363,176,'系统代理接管 · 新连接按当前模式处理',11,'#6e6e73')
    s.button('断开连接',920,140)
    s.parts.append('<path d="M308 202H997" stroke="#ededf0"/>')
    for x,label,value in [(309,'本机流量','系统代理接管'),(550,'路由模式','规则模式'),(790,'当前节点','香港 01 · 36 ms')]:
        s.text(x,226,label,10,'#6e6e73');s.text(x,251,value,12)
    for x in [521,758]: s.text(x,245,'›',20,'#b6b6bc')
    for x,label,value,meta in [(286,'当前下载','2.4 MB/s','本次累计 128 MB'),(535,'当前上传','128 KB/s','本次累计 12 MB'),(784,'活动连接','3','查看会话 ›')]:
        s.rect(x,300,236,100,'#ffffff',9);s.text(x+16,321,label,10,'#6e6e73');s.text(x+16,354,value,23,weight=550);s.text(x+16,381,meta,10,'#6e6e73')
    s.y=420
    s.group('连接设置',[('当前节点','香港 01 · 36 ms  ›'),('流量模式','规则模式  ›'),('系统代理',True),('TUN 模式','未开启  ›')])
    s.text(286,675,'网络检查',12,weight=600);s.text(957,675,'查看诊断',11,'#007aff')
    s.rect(286,690,734,49,'#ffffff',9);s.text(304,720,'● 本地监听已启动',11,'#248a3d');s.text(527,720,'互联网连通性 · 尚未验证',11,'#6e6e73');s.button('测试连接',921,700)
    s.y=756;s.note('服务启动成功不等于目标网站已经连通；所有流量与测速数据均为设计示例。')
    screens.append(s.save('01-overview.svg'))
    s=Screen('节点',1,'选一个节点用于手动出口；自动策略在“策略组”中管理。')
    s.rect(286,111,734,65,'#ffffff',10);s.icon('globe',304,132,'#007aff',24);s.text(343,135,'当前节点',10,'#6e6e73');s.text(343,157,'香港 01',13,weight=600);s.text(798,150,'36 ms',11,'#248a3d');s.button('管理策略组',900,129)
    s.y=196;s.buttons(['搜索名称、地区或协议','测速','＋ 添加'])
    s.text(289,257,'全部来源 ▾       延迟优先 ▾       ☆ 仅收藏',11,'#6e6e73');s.text(942,257,'4 / 4 个节点',10,'#6e6e73')
    s.y=278;s.group('节点列表',[
        ('★  ✓ 香港 01   ·   日常线路 / Shadowsocks','36 ms    详情'),
        ('☆     日本 02   ·   日常线路 / VMess','82 ms    详情'),
        ('☆     新加坡 03   ·   日常线路 / Trojan','108 ms    详情'),
        ('☆     美国 04   ·   备用线路 / VLESS','未测速    详情')])
    s.note('延迟测试 · 最近一次示例测速                         毫秒为延迟，不代表下载带宽')
    s.group('管理',[('订阅来源','2 个来源  ›'),('导出与分享','分享链接与本地订阅  ›')]);s.note('点击节点即选中；“详情”不会切换。切换只影响后续新连接。');screens.append(s.save('02-nodes.svg'))
    s=Screen('订阅',2,'管理节点来源；更新订阅保留你的本地规则。');s.buttons(['＋ 添加订阅','更新全部']);s.group('订阅来源',[('日常线路','3 个节点 · 10 分钟前更新    更新 ›'),('备用线路','1 个节点 · 昨天更新    更新 ›')]);s.note('本地规则独立管理；按来源更新节点，不覆盖分流规则。');s.group('更新设置',[('自动更新','每 6 小时  ›'),('跳过重复节点',True)]);s.note('更新失败也能继续使用：保留上次成功结果，解析通过后再应用。');screens.append(s.save('03-subscriptions.svg'))
    s=Screen('策略组',3,'规则指向策略组，由策略决定实际使用哪个成员节点。');s.buttons(['＋ 新建策略组']);s.group('策略组',[('自动选择 · URL 测速 · 3 个成员','香港 01  ›'),('故障切换 · 按顺序 · 3 个成员','日本 02  ›'),('流媒体 · 手动选择 · 2 个成员','日本 02  ›')]);s.group('全局检测设置',[('自动检测可用性',True),('检测间隔与阈值','300 秒 · 连续失败 3 次  ›')]);s.note('手动选择：固定成员。URL 测速：较低延迟。故障切换：按顺序首个可用。');screens.append(s.save('04-groups.svg'))
    s=Screen('分流规则',4,'查看完整匹配条件，逐条编辑，或直接粘贴批量修改。')
    s.buttons(['搜索规则或出口','状态：全部','批量粘贴','＋ 添加'])
    s.group('国内直连 · 3 条     ＋ 添加 / 批量编辑',[
        ('1   GEOIP · CN','直连    开启    编辑'),
        ('2   IP-CIDR · 192.168.0.0/16','直连    开启    编辑'),
        ('3   DOMAIN-SUFFIX · example.cn','直连    开启    编辑')])
    s.group('跨境访问 · 2 条     ＋ 添加 / 批量编辑',[
        ('4   DOMAIN-SUFFIX · example.org','自动选择    开启    编辑'),
        ('5   DOMAIN-KEYWORD · video','自动选择    开启    编辑')])
    s.group('自定义 · 3 条     ＋ 添加 / 批量编辑',[
        ('6   DOMAIN-SUFFIX · example.com','代理    开启    编辑'),
        ('7   DOMAIN · ads.example.net','拒绝    关闭    编辑'),
        ('8   DST-PORT · 8000-9000','直连    开启    编辑')])
    s.group('默认行为',[('其余流量','自动选择  ›')])
    s.buttons(['预设模板','导出规则','测试匹配'])
    screens.append(s.save('05-rules.svg'))
    s=Screen('连接',5,'查看当前和最近的代理连接。');s.buttons(['搜索目标地址','暂停刷新']);s.group('活动连接 · 24',[('api.example.com:443','SOCKS5 · 活动中  ›'),('cdn.example.com:443','HTTP · 活动中  ›'),('images.example.com:443','SOCKS5 · 活动中  ›')]);s.group('最近关闭',[('docs.example.com:443','↑ 12 KB   ↓ 2.8 MB  ›'),('updates.example.com:443','↑ 4 KB   ↓ 180 KB  ›')]);s.note('连接详情显示开始时间与状态；最终流量在关闭后更新。');screens.append(s.save('06-connections.svg'))
    s=Screen('日志',6,'定位连接和配置问题。');s.buttons(['搜索日志','级别：全部','导出']);s.group('运行日志',[('14:32:08   INFO','代理服务已启动'),('14:32:08   INFO','监听 127.0.0.1:1080 / 8080'),('14:32:10   INFO','节点切换为 香港 01'),('14:33:02   WARN','备用线路更新失败，保留旧节点')]);s.buttons(['暂停滚动','清空日志']);s.note('清空需确认；导出前提示检查个人信息。');screens.append(s.save('07-logs.svg'))
    s=Screen('网络设置',7,'基础选项直接可见，高级参数按需展开。');s.group('本地代理',[('监听地址','127.0.0.1'),('SOCKS5 端口','1080'),('HTTP 端口','8080'),('允许局域网连接',False)]);s.group('流量接管',[('系统代理',True),('TUN 模式','未开启 · 需要管理员权限  ›')]);s.group('高级',[('DNS','Fake IP、上游与缓存  ›'),('出站网络','接口绑定与连接设置  ›')]);s.buttons(['应用更改']);s.note('应用前校验端口范围与冲突；应用时短暂重启监听。');screens.append(s.save('08-network.svg'))
    s=Screen('通用',8,'应用偏好与配置管理。');s.group('偏好设置',[('外观','跟随系统  ›'),('语言','简体中文  ›'),('快捷键','⌘1–⌘9 切换分类  ›')]);s.group('配置',[('导入配置','Clash / sing-box / V2Ray  ›'),('导出配置','选择格式与路径  ›'),('查看配置文件','高级编辑与校验  ›')]);s.group('关于 Sockrocket',[('版本','0.1.2'),('检查更新','手动检查  ›'),('项目主页','GitHub  ↗')]);screens.append(s.save('09-general.svg'))
    s=Screen('概览',0,'首次使用 · 空状态');s.group('添加你的第一个节点',[('1. 添加订阅或节点','支持 6 种协议  ›'),('2. 选择节点与模式','推荐规则模式  ›'),('3. 开启连接','连接后开启系统代理')]);s.buttons(['添加节点与订阅']);s.note('默认只监听本机地址；高级网络设置可以稍后调整。');screens.append(s.save('10-first-run.svg'))
    s=Screen('概览',0,'未连接 · 节点已就绪');s.hero('未连接',False);s.group('代理',[('当前节点','香港 01    36 ms  ›'),('流量模式','规则模式  ›'),('系统代理',False)]);s.note('连接中展示进度并允许取消；断开时恢复原系统代理。');screens.append(s.save('11-disconnected.svg'))
    s=Screen('连接失败',0,'明确原因，提供可以执行的恢复入口。');s.group('无法连接到 香港 01',[('错误原因','连接超时'),('系统代理','已恢复')]);s.group('恢复操作',[('重试当前节点','重新建立连接  ›'),('选择其他节点','4 个可选节点  ›'),('检查网络配置','代理端口与 DNS  ›'),('查看详细日志','错误信息与诊断  ›')]);s.buttons(['重试','选择节点']);screens.append(s.save('12-error.svg'))
    s=Screen('规则详情',4,'规则详情与单条编辑 · 二级视图')
    s.group('匹配条件',[('优先级','6'),('分组','自定义'),('类型','DOMAIN-SUFFIX'),('匹配内容','example.com'),('出口','代理'),('启用此规则',True)])
    s.buttons(['删除','上移','下移','取消','保存'])
    s.note('类型联动格式提示；重复条件、端口、网段和地区代码在保存前校验。')
    screens.append(s.save('13-rule-detail.svg'))
    s=Screen('批量粘贴与编辑',4,'文本编辑 → 校验预览 → 确认保存')
    s.group('编辑范围',[('目标分组','自定义'),('重复规则','跳过重复 / 覆盖已有'),('编辑范围','当前分组 / 所选规则 / 追加导入')])
    s.buttons(['文本编辑','校验预览'])
    s.group('文本编辑 · 每行一条',[('1','DOMAIN-SUFFIX,example.com,PROXY'),('2','GEOIP,CN,DIRECT'),('3','DST-PORT,8000-9000,DIRECT'),('4','MATCH,自动选择')])
    s.note('预览显示新增、覆盖、跳过数量；错误定位到具体行，原始文本保留。')
    s.buttons(['取消','返回修改','确认导入'])
    screens.append(s.save('14-rules-bulk.svg'))
    s=Screen('网络诊断',0,'按服务、接管、节点、目标逐步排查。')
    s.group('1. 本地代理服务',[('本地监听','127.0.0.1:1080 · 已就绪')])
    s.group('2. 流量接管',[('当前方式','系统代理接管 · 已配置')])
    s.group('3. 当前节点',[('香港 01','最近一次延迟 36 ms · 示例')])
    s.group('4. 目标连通性与 DNS',[('验证状态','尚未执行真实验证')])
    s.buttons(['关闭','查看日志','测试匹配']);s.note('以下仅演示状态与恢复入口，不发送真实探测。')
    screens.append(s.save('15-diagnostics.svg'))
    gallery=['<svg xmlns="http://www.w3.org/2000/svg" width="3380" height="4390" viewBox="0 0 3380 4390"><rect width="3380" height="4390" fill="#dce1e9"/><text x="45" y="55" font-family="sans-serif" font-size="28" font-weight="bold" fill="#1d1d1f">Sockrocket · macOS 设置风格</text><text x="45" y="88" font-family="sans-serif" font-size="15" fill="#6e6e73">9 个主页面 + 3 个关键状态 + 2 个规则编辑视图 + 网络诊断 · 产品设计第二版</text>']
    for i,content in enumerate(screens):
        gallery.append(f'<g transform="translate({45+(i%3)*1110} {130+(i//3)*850})">{content}</g>')
    gallery.append('</svg>');(ROOT/'overview.svg').write_text(''.join(gallery))


if __name__=='__main__':
    build()
