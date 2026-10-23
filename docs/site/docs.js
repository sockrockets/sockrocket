'use strict';
const chrome = {
  zh: {
    skip: '跳转到内容',
    navExperience: '产品体验',
    navDownload: '下载',
    navDocs: '文档',
    navCommunity: '社区',
    footer: '开源 Rust 代理客户端 · Apache-2.0',
    docs: '文档',
    home: '首页',
  },
  en: {
    skip: 'Skip to content',
    navExperience: 'Experience',
    navDownload: 'Download',
    navDocs: 'Docs',
    navCommunity: 'Community',
    footer: 'Open-source Rust proxy client · Apache-2.0',
    docs: 'Docs',
    home: 'Home',
  },
  vi: {
    skip: 'Đến nội dung',
    navExperience: 'Trải nghiệm',
    navDownload: 'Tải xuống',
    navDocs: 'Tài liệu',
    navCommunity: 'Cộng đồng',
    footer: 'Ứng dụng proxy Rust mã nguồn mở · Apache-2.0',
    docs: 'Tài liệu',
    home: 'Trang chủ',
  },
};

const titleByPage = {
  docs: {
    zh: '文档 · Sockrocket',
    en: 'Docs · Sockrocket',
    vi: 'Tài liệu · Sockrocket',
  },
  'getting-started': {
    zh: '使用指南 · Sockrocket',
    en: 'Getting started · Sockrocket',
    vi: 'Bắt đầu · Sockrocket',
  },
  merlin: {
    zh: 'Merlin 安装指南 · Sockrocket',
    en: 'Merlin install guide · Sockrocket',
    vi: 'Hướng dẫn Merlin · Sockrocket',
  },
  configuration: {
    zh: '配置说明 · Sockrocket',
    en: 'Configuration · Sockrocket',
    vi: 'Cấu hình · Sockrocket',
  },
  protocols: {
    zh: '协议与传输 · Sockrocket',
    en: 'Protocols · Sockrocket',
    vi: 'Giao thức · Sockrocket',
  },
};

let locale = 'zh';
const page = document.body.dataset.page || 'docs';

function contentLocale(value) {
  return value === 'zh' ? 'zh' : 'en';
}

function setLanguage(value) {
  locale = Object.hasOwn(chrome, value) ? value : 'zh';
  const t = chrome[locale];
  document.querySelectorAll('[data-t]').forEach((el) => {
    if (Object.hasOwn(t, el.dataset.t)) el.innerHTML = t[el.dataset.t];
  });
  const show = contentLocale(locale);
  document.querySelectorAll('.doc-locale').forEach((block) => {
    block.hidden = block.dataset.locale !== show;
  });
  document.documentElement.lang = { zh: 'zh-CN', en: 'en', vi: 'vi' }[locale];
  const titles = titleByPage[page] || titleByPage.docs;
  document.title = titles[locale] || titles.en;
  const select = document.getElementById('language');
  if (select) select.value = locale;
  try {
    localStorage.setItem('sockrocket-language', locale);
  } catch {}
}

let saved;
try {
  saved = localStorage.getItem('sockrocket-language');
} catch {}
setLanguage(
  saved ||
    (navigator.language.startsWith('zh')
      ? 'zh'
      : navigator.language.startsWith('vi')
        ? 'vi'
        : 'en'),
);
document.getElementById('language')?.addEventListener('change', (e) => {
  setLanguage(e.target.value);
});

document.querySelectorAll('[data-copy]').forEach((button) => {
  button.addEventListener('click', async () => {
    const target = document.getElementById(button.dataset.copy);
    if (!target) return;
    try {
      await navigator.clipboard.writeText(target.textContent);
      const previous = button.textContent;
      button.textContent = locale === 'zh' ? '已复制' : locale === 'vi' ? 'Đã sao chép' : 'Copied';
      setTimeout(() => {
        button.textContent = previous;
      }, 1600);
    } catch {}
  });
});
