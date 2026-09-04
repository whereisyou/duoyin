import { describe, it, expect } from "vitest";
import { fmtSrtTime, fmtTime, buildSrt, parseSrt } from "../srt";

describe("fmtSrtTime 时间戳格式化", () => {
  it("零值", () => {
    expect(fmtSrtTime(0)).toBe("00:00:00,000");
  });

  it("常规秒数", () => {
    expect(fmtSrtTime(1.5)).toBe("00:00:01,500");
    expect(fmtSrtTime(61.25)).toBe("00:01:01,250");
  });

  it("跨小时", () => {
    expect(fmtSrtTime(3661.5)).toBe("01:01:01,500");
  });

  it("毫秒四舍五入进位，不产生 ,1000 非法时间戳", () => {
    // 1.9995s → 1999.5ms → round=2000ms → 00:00:02,000
    expect(fmtSrtTime(1.9995)).toBe("00:00:02,000");
  });

  it("负数钳制为 0", () => {
    expect(fmtSrtTime(-3)).toBe("00:00:00,000");
  });
});

describe("fmtTime 预览格式", () => {
  it("常规与补零", () => {
    expect(fmtTime(0)).toBe("00:00:00.0");
    expect(fmtTime(61.5)).toBe("00:01:01.5");
  });
});

describe("buildSrt 字幕生成", () => {
  const segs = [
    { idx: 0, start: 0, end: 1.5, text: "hello", translated: "你好" },
    { idx: 1, start: 2, end: 3, text: "world", translated: "" },
  ];

  it("仅译文模式：空译文回退原文", () => {
    const srt = buildSrt(segs, false);
    expect(srt).toContain("1\n00:00:00,000 --> 00:00:01,500\n你好");
    expect(srt).toContain("2\n00:00:02,000 --> 00:00:03,000\nworld");
  });

  it("双语模式：原文 + 译文两行", () => {
    const srt = buildSrt([segs[0]], true);
    expect(srt).toContain("hello\n你好");
  });

  it("双语模式下原文为空的段不插入空行", () => {
    const srt = buildSrt([{ idx: 0, start: 0, end: 1, text: "  ", translated: "你好" }], true);
    expect(srt).toContain("\n你好\n");
    expect(srt).not.toContain("\n\n\n");
  });

  it("编号从 1 连续递增", () => {
    const srt = buildSrt(segs, false);
    expect(srt.startsWith("1\n")).toBe(true);
    expect(srt).toMatch(/\n\n2\n/);
  });
});

describe("parseSrt 字幕解析", () => {
  it("标准块解析：时间与多行文本", () => {
    const segs = parseSrt("1\n00:00:01,000 --> 00:00:02,500\nline1\nline2");
    expect(segs).toHaveLength(1);
    expect(segs[0]).toMatchObject({
      idx: 0,
      start: 1,
      end: 2.5,
      text: "line1\nline2",
      translated: "",
    });
  });

  it("BOM 与 CRLF 兼容", () => {
    const segs = parseSrt("\uFEFF1\r\n00:00:00,000 --> 00:00:01,000\r\nhi");
    expect(segs).toHaveLength(1);
    expect(segs[0].text).toBe("hi");
  });

  it("点号毫秒分隔符兼容（部分工具导出格式）", () => {
    const segs = parseSrt("1\n00:00:01.000 --> 00:00:02.000\nx");
    expect(segs[0]).toMatchObject({ start: 1, end: 2 });
  });

  it("无效时间行与残块跳过，不产生空段", () => {
    const segs = parseSrt("随便一行\n没有时间轴\n\n1\n00:00:01,000 --> 00:00:02,000\nok");
    expect(segs).toHaveLength(1);
    expect(segs[0].text).toBe("ok");
  });

  it("idx 按解析顺序重排（与源编号无关）", () => {
    const segs = parseSrt("9\n00:00:01,000 --> 00:00:02,000\na\n\n3\n00:00:03,000 --> 00:00:04,000\nb");
    expect(segs.map((s) => s.idx)).toEqual([0, 1]);
  });
});

describe("SRT 生成-解析 round-trip", () => {
  it("时间信息守恒", () => {
    const segs = [
      { idx: 0, start: 12.345, end: 67.89, text: "原文", translated: "译文" },
      { idx: 1, start: 100.001, end: 101.999, text: "a", translated: "b" },
    ];
    const parsed = parseSrt(buildSrt(segs, false));
    expect(parsed.map((s) => s.start)).toEqual(segs.map((s) => s.start));
    expect(parsed.map((s) => s.end)).toEqual(segs.map((s) => s.end));
    expect(parsed.map((s) => s.text)).toEqual(["译文", "b"]);
  });
});
