/**
 * 工作台交互纯函数（从 HomePage.vue 抽取，vitest 直测）。
 * HomePage 只保留 UI 绑定，业务规则在这里沉淀。
 */
import type { AppConfig, LanguageDialectSpec, TargetVariant } from "./types";
import { languageVariant, dialectVariant } from "./langs";

/**
 * 目标语言 + 方言选择 → 实际要生成的版本列表。
 * 规则：非中文语言直接映射；中文按勾选方言展开（无勾选回退普通话）；
 * 未知方言 id（规格未加载/已下架）静默跳过。
 */
export function expandTargetVariants(
  targetLangs: string[],
  dialectsByLanguage: Record<string, string[]>,
  dialectSpecs: LanguageDialectSpec[],
): TargetVariant[] {
  const variants: TargetVariant[] = [];
  for (const language of targetLangs) {
    if (language !== "zh") {
      variants.push(languageVariant(language));
      continue;
    }
    const selected = dialectsByLanguage.zh;
    if (!selected?.length) {
      variants.push(languageVariant("zh"));
      continue;
    }
    const spec = dialectSpecs.find((item) => item.language === "zh");
    for (const id of selected) {
      const dialect = spec?.dialects.find((item) => item.id === id);
      if (dialect) variants.push(dialectVariant("zh", dialect));
    }
  }
  return variants;
}

export interface EnhancementTag {
  key: string;
  label: string;
  /** true = 影响处理链路的增强（主题色强调）；false = 输出形态选项（中性色） */
  enhanced: boolean;
}

/** 已启用的高阶功能标签（高级设置入口旁常驻显示，不打开弹窗也可见）。 */
export function enabledEnhancementTags(config: AppConfig): EnhancementTag[] {
  const tags: EnhancementTag[] = [];
  if (config.separation_enabled) {
    tags.push({ key: "separation", label: "背景分离", enhanced: true });
  }
  if (config.tts_use_video_prompt) {
    tags.push({ key: "voice_clone", label: "原声克隆", enhanced: true });
  }
  if (config.keep_original_audio_track) {
    tags.push({ key: "dual_track", label: "双音轨", enhanced: false });
  }
  if (config.generate_final_videos) {
    tags.push({ key: "final_video", label: "含成片", enhanced: false });
  }
  return tags;
}

/**
 * 高级设置草稿数值归一（与后端 save_config 的 clamp 规则对齐，
 * 保证前端 store.config 与落盘值一致）。
 */
export function normalizeAdvancedDraft(config: AppConfig): AppConfig {
  return {
    ...config,
    api_max_concurrent: clamp(Math.round(config.api_max_concurrent), 1, 16),
    api_interval_ms: Math.min(Math.round(config.api_interval_ms), 600_000),
    min_speed_percent: clamp(Math.round(config.min_speed_percent), 50, 100),
    max_speed_percent: clamp(Math.round(config.max_speed_percent), 100, 200),
  };
}

/** 高级设置草稿校验：返回错误文案（阻断保存），null 表示通过。 */
export function validateAdvancedDraft(config: AppConfig): string | null {
  if (config.min_speed_percent > config.max_speed_percent) {
    return "最小变速比例不能大于最大比例";
  }
  return null;
}

function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}
