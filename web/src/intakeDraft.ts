// 表单设计器里的自动命名与类型推断。
//
// 这些规则只在前端：配置层只认最终写进去的 `slug` / `name`，服务端不知道某个
// 名字是不是跟着标题生成的。所以把判断做成纯函数，单独测。
//
// 中文标题没有可靠的转写：slug 要进 URL，只能留 ASCII，标题里没有拉丁字母时
// 由调用方换一个占位词；字段名不进 URL，可以保留中文，保留原样比造一个编号好认。

import type { IntakeFieldType } from "./intakeApi";

const MAX_SLUG = 64;
const MAX_NAME = 64;

/// 标题压成 URL 用的 slug：只留小写字母、数字和短横线。
export function slugify(text: string): string {
  return text
    .normalize("NFKD")
    .replace(/\p{M}/gu, "")
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, MAX_SLUG)
    .replace(/-+$/, "");
}

/// 字段名从显示名来：保留中文和字母数字，空白与标点折成短横线。
export function fieldNameFrom(label: string): string {
  return label
    .normalize("NFKD")
    .replace(/\p{M}/gu, "")
    .toLowerCase()
    .replace(/[^\p{L}\p{N}]+/gu, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, MAX_NAME)
    .replace(/-+$/, "");
}

/// 在已占用的名字里找一个空位：`base`、`base-2`、`base-3`……
/// 后缀也算在长度上限里，免得截断之后又撞名。
export function uniqueName(base: string, taken: Set<string>): string {
  if (!base) return "";
  let candidate = base;
  let index = 2;
  while (taken.has(candidate)) {
    const suffix = `-${index}`;
    candidate = `${base.slice(0, MAX_NAME - suffix.length)}${suffix}`;
    index += 1;
  }
  return candidate;
}

/// 按显示名猜类型。只是新字段的默认值；用户自己选过类型之后就不再改。
/// 顺序有讲究：「手机号码」要先按电话认，「邮箱地址」要先按邮箱认。
const TYPE_KEYWORDS: Array<[IntakeFieldType, RegExp]> = [
  ["email", /邮箱|电邮|电子邮件|邮件地址|email|e-mail|mail/],
  ["tel", /电话|手机|座机|联系方式|phone|mobile|tel\b|telephone|cell/],
  ["url", /网址|网站|链接|主页|官网|博客|url|website|homepage|link|blog/],
  ["date", /日期|生日|出生|date|birthday|dob/],
  ["number", /年龄|数量|金额|价格|数字|分数|人数|工资|薪水|预算|次数|age|count|quantity|amount|price|number|score|budget|weight|height/],
  ["textarea", /留言|内容|备注|描述|说明|详情|简介|反馈|建议|message|comment|note|description|detail|feedback|content|body/],
];

export function inferFieldType(label: string): IntakeFieldType | null {
  const text = label.trim().toLowerCase();
  if (!text) return null;
  for (const [type, pattern] of TYPE_KEYWORDS) {
    if (pattern.test(text)) return type;
  }
  return null;
}
