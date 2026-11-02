import { expect, it } from "vitest";
import { fieldNameFrom, inferFieldType, slugify, uniqueName } from "./intakeDraft";

it("slugify keeps latin words and drops what a URL cannot carry", () => {
  expect(slugify("Contact Us!")).toBe("contact-us");
  expect(slugify("  Réunion 2026 ")).toBe("reunion-2026");
  expect(slugify("你的信息")).toBe("");
  expect(slugify("Hello 世界 2")).toBe("hello-2");
  expect(slugify("a".repeat(80))).toHaveLength(64);
  expect(slugify("--x--")).toBe("x");
});

it("fieldNameFrom keeps Chinese because field names do not go into URLs", () => {
  expect(fieldNameFrom("Phone Number")).toBe("phone-number");
  expect(fieldNameFrom("年龄（岁）")).toBe("年龄-岁");
  expect(fieldNameFrom("  ")).toBe("");
});

it("uniqueName appends the first free number", () => {
  expect(uniqueName("contact", new Set())).toBe("contact");
  expect(uniqueName("contact", new Set(["contact"]))).toBe("contact-2");
  expect(uniqueName("contact", new Set(["contact", "contact-2"]))).toBe("contact-3");
  expect(uniqueName("", new Set(["x"]))).toBe("");
  const long = uniqueName("a".repeat(64), new Set(["a".repeat(64)]));
  expect(long.length).toBeLessThanOrEqual(64);
  expect(long.endsWith("-2")).toBe(true);
});

it("infers common field types from the label", () => {
  expect(inferFieldType("邮箱")).toBe("email");
  expect(inferFieldType("手机号码")).toBe("tel");
  expect(inferFieldType("电话")).toBe("tel");
  expect(inferFieldType("年龄")).toBe("number");
  expect(inferFieldType("留言")).toBe("textarea");
  expect(inferFieldType("网址")).toBe("url");
  expect(inferFieldType("生日")).toBe("date");
  expect(inferFieldType("姓名")).toBeNull();
  expect(inferFieldType("")).toBeNull();
});
