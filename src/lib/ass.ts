import type { SubtitleSegment } from "./types";

export interface AssStyle {
  fontName: string;
  fontSize: number;
  primaryColor: string;
  outlineColor: string;
  outline: number;
  marginV: number;
}

function assTime(seconds: number): string {
  const h = Math.floor(seconds / 3600);
  const m = Math.floor((seconds % 3600) / 60);
  const s = (seconds % 60).toFixed(2).padStart(5, "0");
  return `${h}:${m.toString().padStart(2, "0")}:${s}`;
}

function assColor(hex: string): string {
  const value = hex.replace("#", "").padEnd(6, "F");
  return `&H00${value.slice(4, 6)}${value.slice(2, 4)}${value.slice(0, 2).toUpperCase()}`;
}

export function buildAss(segments: SubtitleSegment[], style: AssStyle): string {
  const header = `[Script Info]
ScriptType: v4.00+
PlayResX: 1920
PlayResY: 1080
ScaledBorderAndShadow: yes

[V4+ Styles]
Format: Name,Fontname,Fontsize,PrimaryColour,SecondaryColour,OutlineColour,BackColour,Bold,Italic,Underline,StrikeOut,ScaleX,ScaleY,Spacing,Angle,BorderStyle,Outline,Shadow,Alignment,MarginL,MarginR,MarginV,Encoding
Style: Default,${style.fontName},${style.fontSize},${assColor(style.primaryColor)},&H000000FF,${assColor(style.outlineColor)},&H80000000,0,0,0,0,100,100,0,0,1,${style.outline},0,2,40,40,${style.marginV},1

[Events]
Format: Layer,Start,End,Style,Name,MarginL,MarginR,MarginV,Effect,Text`;
  const events = segments.map((segment) => {
    const text = (segment.translated || segment.text).replace(/\n/g, "\\N").replace(/[{}]/g, "");
    return `Dialogue: 0,${assTime(segment.start)},${assTime(segment.end)},Default,,0,0,0,,${text}`;
  });
  return `${header}\n${events.join("\n")}\n`;
}
