import type { SubtitleSegment } from "./types";

/** 秒 → SRT 时间戳 00:00:01,500 */
export function fmtSrtTime(sec: number): string {
  // 先取整到总毫秒再分解，避免小数毫秒四舍五入产生 ,1000 的非法时间戳
  const total = Math.max(0, Math.round(sec * 1000));
  const h = Math.floor(total / 3600000);
  const m = Math.floor((total % 3600000) / 60000);
  const s = Math.floor((total % 60000) / 1000);
  const ms = total % 1000;
  const p = (n: number, w = 2) => n.toString().padStart(w, "0");
  return `${p(h)}:${p(m)}:${p(s)},${p(ms, 3)}`;
}

/** 秒 → 简短时间 00:00:01.5（预览用） */
export function fmtTime(sec: number): string {
  const s = Math.max(0, sec);
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const rest = (s % 60).toFixed(1);
  const p = (n: number) => n.toString().padStart(2, "0");
  return `${p(h)}:${p(m)}:${rest.padStart(4, "0")}`;
}

/**
 * 生成 SRT 文本
 * @param bilingual true 时输出 原文+译文 双语字幕，false 仅译文
 */
export function buildSrt(segments: SubtitleSegment[], bilingual: boolean): string {
  return segments
    .map((seg, i) => {
      const lines: string[] = [];
      if (bilingual && seg.text?.trim()) lines.push(seg.text.trim());
      const trans = seg.translated?.trim();
      lines.push(trans || seg.text.trim());
      return `${i + 1}\n${fmtSrtTime(seg.start)} --> ${fmtSrtTime(seg.end)}\n${lines.join("\n")}`;
    })
    .join("\n\n") + "\n";
}

/** 解析 SRT 文本为字幕段（译文栏留空） */
export function parseSrt(content: string): SubtitleSegment[] {
  const blocks = content.replace(/^\uFEFF/, "").replace(/\r/g, "").split(/\n\n+/);
  const segments: SubtitleSegment[] = [];
  for (const block of blocks) {
    const lines = block.trim().split("\n");
    if (lines.length < 2) continue;
    const timeLine = lines[1];
    const m = timeLine.match(
      /(\d{2}):(\d{2}):(\d{2})[,.](\d{3})\s*-->\s*(\d{2}):(\d{2}):(\d{2})[,.](\d{3})/,
    );
    if (!m) continue;
    const toSec = (h: string, m2: string, s: string, ms: string) =>
      Number(h) * 3600 + Number(m2) * 60 + Number(s) + Number(ms) / 1000;
    segments.push({
      idx: segments.length,
      start: toSec(m[1], m[2], m[3], m[4]),
      end: toSec(m[5], m[6], m[7], m[8]),
      text: lines.slice(2).join("\n"),
      translated: "",
    });
  }
  return segments;
}
