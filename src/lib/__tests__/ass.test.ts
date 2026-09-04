import { describe, it, expect } from "vitest";
import { buildAss, type AssStyle } from "../ass";

const style: AssStyle = {
  fontName: "Arial",
  fontSize: 48,
  primaryColor: "#FFFFFF",
  outlineColor: "#000000",
  outline: 2,
  marginV: 40,
};

describe("buildAss 字幕生成", () => {
  it("包含三段必备区块", () => {
    const ass = buildAss([], style);
    expect(ass).toContain("[Script Info]");
    expect(ass).toContain("[V4+ Styles]");
    expect(ass).toContain("[Events]");
  });

  it("Style 行携带字体/字号/描边参数", () => {
    const ass = buildAss([], style);
    expect(ass).toContain("Style: Default,Arial,48,");
    expect(ass).toContain(",1,2,0,2,40,40,40,1");
  });

  it("颜色按 ASS 的 BGR 序转换（#FF0000 红 → &H000000FF）", () => {
    const ass = buildAss([], { ...style, primaryColor: "#FF0000" });
    expect(ass).toContain("&H000000FF");
  });

  it("Dialogue 时间格式 H:MM:SS.cc，译文优先于原文", () => {
    const ass = buildAss(
      [{ idx: 0, start: 1.5, end: 61.25, text: "原文", translated: "译文" }],
      style,
    );
    expect(ass).toContain("Dialogue: 0,0:00:01.50,0:01:01.25,Default,,0,0,0,,译文");
  });

  it("无译文时回退原文", () => {
    const ass = buildAss(
      [{ idx: 0, start: 0, end: 1, text: "原文", translated: "" }],
      style,
    );
    expect(ass).toContain(",,原文");
  });

  it("文本换行转 \\N，花括号剥离（防 ASS 标签注入）", () => {
    const ass = buildAss(
      [{ idx: 0, start: 0, end: 1, text: "a\nb", translated: "{\\pos(1,2)}c\\Nd" }],
      style,
    );
    const dialogue = ass.split("\n").find((l) => l.startsWith("Dialogue"))!;
    expect(dialogue).toContain("c\\Nd");
    expect(dialogue).not.toContain("{");
  });
});
