/**
 * 引擎注册表契约测试：防止「设置页显示即将支持/字段写不进配置」这类
 * 纯数据对齐问题——这类 bug 类型检查抓不到，运行时才发现。
 */
import { describe, it, expect } from "vitest";
import type { AppConfig } from "../types";
import {
  STT_ENGINES,
  TRANSLATE_ENGINES,
  TTS_ENGINES,
  DEFAULT_CONFIG,
  engineById,
} from "../engines";

const ALL_ENGINES = [...STT_ENGINES, ...TRANSLATE_ENGINES, ...TTS_ENGINES];

describe("引擎注册表契约", () => {
  it("引擎 id 全局唯一（设置页按 id 驱动，重复会串配置）", () => {
    const ids = ALL_ENGINES.map((e) => e.id);
    expect(new Set(ids).size).toBe(ids.length);
  });

  it("所有表单字段 key 都存在于 DEFAULT_CONFIG（否则输入写不进配置对象）", () => {
    const configKeys = new Set(Object.keys(DEFAULT_CONFIG));
    for (const engine of ALL_ENGINES) {
      for (const field of engine.fields) {
        expect(
          configKeys.has(field.key as string),
          `引擎 ${engine.id} 的字段 ${String(field.key)} 不在 DEFAULT_CONFIG 中`,
        ).toBe(true);
      }
    }
  });

  it("每个引擎至少有一个字段，且 ready() 对默认配置返回布尔", () => {
    for (const engine of ALL_ENGINES) {
      expect(engine.fields.length, `引擎 ${engine.id} 没有字段`).toBeGreaterThan(0);
      expect(typeof engine.ready(DEFAULT_CONFIG)).toBe("boolean");
    }
  });

  it("路径全空时所有引擎都未就绪（不给用户假绿灯）", () => {
    // 默认配置现在携带真实的本地模型路径（见 engines.ts 顶部注释），
    // 假绿灯防范的对象是“空路径 / 未配置”——把字符串字段全部清空再断言。
    const unset: Record<string, unknown> = { ...DEFAULT_CONFIG };
    for (const key of Object.keys(unset)) {
      if (typeof unset[key] === "string") unset[key] = "";
    }
    for (const engine of ALL_ENGINES) {
      expect(engine.ready(unset as unknown as AppConfig), `引擎 ${engine.id} 空路径下不应可用`).toBe(false);
    }
  });
});

describe("外部 API 连通测试配置", () => {
  const testableFields = ALL_ENGINES.flatMap((e) => e.fields).filter((f) => f.testable);

  it("每个外部 API 都有可测试字段（用户要求：配置变更能验通路）", () => {
    // deepseek(翻译) / openai(STT) / cosyvoice(TTS) 三个外部服务都要可测
    const ids = ALL_ENGINES.filter((e) => e.fields.some((f) => f.testable)).map((e) => e.id);
    expect(ids).toContain("deepseek");
    expect(ids).toContain("openai_api");
    expect(ids).toContain("cosyvoice3");
  });

  it("testable 字段能解析出测试地址（testUrl 或字段本身就是 URL）", () => {
    for (const f of testableFields) {
      const hasUrlSource = !!f.testUrl || f.key.toString().includes("url");
      expect(hasUrlSource, `字段 ${String(f.key)} 既无 testUrl 也不是 URL 字段，测不了`).toBe(true);
    }
  });

  it("testMode 只能是 chat / reachable / 缺省", () => {
    for (const f of testableFields) {
      expect([undefined, "chat", "reachable"]).toContain(f.testMode);
    }
  });
});

describe("sensevoice 引擎（新接入）", () => {
  it("已注册且是 STT 列表成员", () => {
    const sv = engineById(STT_ENGINES, "sensevoice");
    expect(sv.id).toBe("sensevoice");
  });

  it("ready 只看 sensevoice_dir 是否填写", () => {
    const sv = engineById(STT_ENGINES, "sensevoice");
    expect(sv.ready({ ...DEFAULT_CONFIG, sensevoice_dir: "" })).toBe(false);
    expect(sv.ready({ ...DEFAULT_CONFIG, sensevoice_dir: "E:/models/sv" })).toBe(true);
  });

  it("老引擎未被破坏：whisper_native / openai_api 的 ready 逻辑不变", () => {
    expect(engineById(STT_ENGINES, "whisper_native").ready({
      ...DEFAULT_CONFIG, whisper_model_dir: "E:/m",
    })).toBe(true);
    expect(engineById(STT_ENGINES, "openai_api").ready({
      ...DEFAULT_CONFIG, openai_key: "sk-x",
    })).toBe(true);
  });
});
