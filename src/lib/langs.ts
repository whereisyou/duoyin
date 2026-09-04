/** 语言选项（与 whisper 语言代码对齐） */
import type { DialectSpec, TargetVariant } from "./types";

export const SOURCE_LANGS: { label: string; value: string }[] = [
  { label: "自动识别", value: "auto" },
];

export const LANGS: { label: string; value: string }[] = [
  { label: "中文", value: "zh" },
  { label: "英语", value: "en" },
  { label: "日语", value: "ja" },
  { label: "韩语", value: "ko" },
  { label: "法语", value: "fr" },
  { label: "德语", value: "de" },
  { label: "西班牙语", value: "es" },
  { label: "俄语", value: "ru" },
];

export function langLabel(v: string): string {
  if (v === "auto") return "自动识别";
  return LANGS.find((o) => o.value === v)?.label ?? v;
}

export function languageVariant(code: string): TargetVariant {
  if (code === "zh") {
    return {
      id: "zh-CN",
      language: "zh",
      dialect: "mandarin",
      display_name: "中文（普通话）",
      translate_style: "普通话",
      tts_accent: "请用普通话表达。",
    };
  }
  return {
    id: code,
    language: code,
    display_name: langLabel(code),
    translate_style: "",
    tts_accent: "",
  };
}

export function dialectVariant(language: string, dialect: DialectSpec): TargetVariant {
  return {
    id: `${language}-${dialect.id}`,
    language,
    dialect: dialect.id,
    display_name: `${langLabel(language)}（${dialect.label}）`,
    translate_style: dialect.translate_style,
    tts_accent: dialect.tts_accent,
  };
}
