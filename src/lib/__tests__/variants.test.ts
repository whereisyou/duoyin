import { describe, it, expect } from "vitest";
import {
  expandTargetVariants,
  enabledEnhancementTags,
  normalizeAdvancedDraft,
  validateAdvancedDraft,
} from "../variants";
import { DEFAULT_CONFIG } from "../engines";
import type { LanguageDialectSpec } from "../types";

const specs: LanguageDialectSpec[] = [
  {
    language: "zh",
    dialects: [
      { id: "mandarin", label: "普通话", translate_style: "普通话", tts_accent: "请用普通话表达。" },
      { id: "yue", label: "粤语", translate_style: "粤语口语", tts_accent: "请用粤语表达。" },
    ],
  },
];

describe("expandTargetVariants 方言展开", () => {
  it("非中文语言直接映射，不带方言", () => {
    const variants = expandTargetVariants(["en", "ja"], {}, specs);
    expect(variants).toHaveLength(2);
    expect(variants[0]).toMatchObject({ id: "en", language: "en", display_name: "英语" });
    expect(variants[1]).toMatchObject({ id: "ja", language: "ja" });
  });

  it("中文无勾选时回退普通话单版本", () => {
    const variants = expandTargetVariants(["zh"], {}, specs);
    expect(variants).toHaveLength(1);
    expect(variants[0]).toMatchObject({ id: "zh-CN", dialect: "mandarin" });
  });

  it("中文勾选空数组同样回退普通话", () => {
    const variants = expandTargetVariants(["zh"], { zh: [] }, specs);
    expect(variants).toHaveLength(1);
    expect(variants[0].dialect).toBe("mandarin");
  });

  it("中文多方言展开为多版本，id 前缀 language-dialect", () => {
    const variants = expandTargetVariants(["zh"], { zh: ["yue", "mandarin"] }, specs);
    // 显式勾选走 dialectVariant（zh-mandarin）；zh-CN 仅在无勾选回退分支生成（见下一用例）
    expect(variants.map((v) => v.id)).toEqual(["zh-yue", "zh-mandarin"]);
    expect(variants[0]).toMatchObject({
      language: "zh",
      dialect: "yue",
      translate_style: "粤语口语",
    });
  });

  it("混合目标：外语 + 中文方言各自展开", () => {
    const variants = expandTargetVariants(["en", "zh"], { zh: ["yue"] }, specs);
    expect(variants.map((v) => v.id)).toEqual(["en", "zh-yue"]);
  });

  it("未知方言 id（规格未加载）静默跳过", () => {
    const variants = expandTargetVariants(["zh"], { zh: ["nonexistent"] }, specs);
    expect(variants).toHaveLength(0);
  });

  it("方言规格为空列表时勾选也跳过", () => {
    const variants = expandTargetVariants(["zh"], { zh: ["yue"] }, []);
    expect(variants).toHaveLength(0);
  });
});

describe("enabledEnhancementTags 高阶功能常驻标签", () => {
  // DEFAULT_CONFIG 默认含成片（generate_final_videos: true），测「全关」需显式置 false
  const allOff = { ...DEFAULT_CONFIG, generate_final_videos: false };

  it("全部关闭时为空（入口显示默认提示文案）", () => {
    expect(enabledEnhancementTags(allOff)).toHaveLength(0);
  });

  it("链路增强类（分离/原声克隆）标记 enhanced", () => {
    const tags = enabledEnhancementTags({
      ...allOff,
      separation_enabled: true,
      tts_use_video_prompt: true,
    });
    expect(tags).toEqual([
      { key: "separation", label: "背景分离", enhanced: true },
      { key: "voice_clone", label: "原声克隆", enhanced: true },
    ]);
  });

  it("输出形态类（双音轨/含成片）不标记 enhanced", () => {
    const tags = enabledEnhancementTags({
      ...DEFAULT_CONFIG,
      keep_original_audio_track: true,
      generate_final_videos: true,
    });
    expect(tags.map((t) => t.enhanced)).toEqual([false, false]);
  });

  it("四项全开时顺序稳定", () => {
    const tags = enabledEnhancementTags({
      ...DEFAULT_CONFIG,
      separation_enabled: true,
      tts_use_video_prompt: true,
      keep_original_audio_track: true,
      generate_final_videos: true,
    });
    expect(tags.map((t) => t.key)).toEqual([
      "separation",
      "voice_clone",
      "dual_track",
      "final_video",
    ]);
  });
});

describe("normalizeAdvancedDraft 数值归一（与后端 clamp 对齐）", () => {
  it("变速范围 clamp 到 [50,100] / [100,200]", () => {
    const out = normalizeAdvancedDraft({
      ...DEFAULT_CONFIG,
      min_speed_percent: 10,
      max_speed_percent: 300,
    });
    expect(out.min_speed_percent).toBe(50);
    expect(out.max_speed_percent).toBe(200);
  });

  it("API 并发 clamp 到 [1,16]，间隔封顶 600000ms", () => {
    const out = normalizeAdvancedDraft({
      ...DEFAULT_CONFIG,
      api_max_concurrent: 99,
      api_interval_ms: 999_999,
    });
    expect(out.api_max_concurrent).toBe(16);
    expect(out.api_interval_ms).toBe(600_000);
  });

  it("合法值原样保留且不修改原对象", () => {
    const draft = { ...DEFAULT_CONFIG, min_speed_percent: 90, max_speed_percent: 120 };
    const out = normalizeAdvancedDraft(draft);
    expect(out.min_speed_percent).toBe(90);
    expect(out.max_speed_percent).toBe(120);
    expect(draft.min_speed_percent).toBe(90);
    expect(out).not.toBe(draft);
  });
});

describe("validateAdvancedDraft 草稿校验", () => {
  it("min > max 返回错误文案", () => {
    expect(
      validateAdvancedDraft({ ...DEFAULT_CONFIG, min_speed_percent: 110, max_speed_percent: 105 }),
    ).toBe("最小变速比例不能大于最大比例");
  });

  it("min == max 边界通过（clamp 后不可能，但防御性放行）", () => {
    expect(
      validateAdvancedDraft({ ...DEFAULT_CONFIG, min_speed_percent: 100, max_speed_percent: 100 }),
    ).toBeNull();
  });

  it("正常范围通过", () => {
    expect(
      validateAdvancedDraft({ ...DEFAULT_CONFIG, min_speed_percent: 85, max_speed_percent: 125 }),
    ).toBeNull();
  });
});
