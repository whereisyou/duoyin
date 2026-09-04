// 真机回归一键脚本：预检本地模型/工具 → 跑全部 --ignored 测试 → 汇总报告。
// 用法：npm run e2e        （项目根目录，无需进 src-tauri）
// 模型路径可用环境变量覆盖：VT_WHISPER_DIR / VT_SUPERTONIC_DIR / VT_SENSEVOICE_DIR /
//                           VT_ZIPVOICE_DIR / VT_UVR_MODEL / VT_TEST_AUDIO
import { spawnSync } from "node:child_process";
import { existsSync, statSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const checks = [
  {
    name: "ffmpeg",
    ok: () => spawnSync("ffmpeg", ["-version"], { stdio: "ignore" }).status === 0,
    hint: "安装 ffmpeg 并加入 PATH",
  },
  {
    name: "ffprobe",
    ok: () => spawnSync("ffprobe", ["-version"], { stdio: "ignore" }).status === 0,
    hint: "安装 ffprobe 并加入 PATH",
  },
  {
    name: "Whisper 模型 (VT_WHISPER_DIR)",
    ok: () => {
      const dir =
        process.env.VT_WHISPER_DIR ??
        "E:/projects/text2voices/CosyVoice/pretrained_models/whisper-large-v3-turbo";
      return existsSync(dir) && statSync(dir).isDirectory();
    },
    hint: "e2e: TTS→STT 跨窗回归 / Whisper 契约（缺 → 相关用例跳过）",
  },
  {
    name: "Supertonic 模型 (VT_SUPERTONIC_DIR)",
    ok: () => {
      const dir =
        process.env.VT_SUPERTONIC_DIR ?? "E:/projects/pyvideotrans-3.98/Supertone/supertonic-3";
      return existsSync(dir) && statSync(dir).isDirectory();
    },
    hint: "e2e TTS 合成（缺 → 多数真机用例无法合成语音）",
  },
  {
    name: "SenseVoice 模型 (VT_SENSEVOICE_DIR)",
    ok: () => {
      const dir = process.env.VT_SENSEVOICE_DIR ?? "E:/projects/test2voices_backup/sense-voice-int8";
      return existsSync(dir) && statSync(dir).isDirectory();
    },
    hint: "application 全链路测试用（缺 → 该用例跳过）",
  },
  {
    name: "ZipVoice 模型 (VT_ZIPVOICE_DIR)",
    ok: () => {
      const dir =
        process.env.VT_ZIPVOICE_DIR ??
        "E:/projects/test2voices_backup/sherpa-onnx-zipvoice-distill-int8-zh-en-emilia";
      return existsSync(dir) && statSync(dir).isDirectory();
    },
    hint: "ZipVoice 真机克隆测试（缺 → 跳过）",
  },
  {
    name: "测试视频 10.mp4 (VT_TEST_VIDEO)",
    ok: () => {
      const file = process.env.VT_TEST_VIDEO ?? "E:/projects/pyvideotrans-3.98/10.mp4";
      return existsSync(file) && statSync(file).isFile();
    },
    hint: "场景矩阵测试素材（缺 → 9 个场景用例跳过）",
  },
  {
    name: "UVR 分离模型 (VT_UVR_MODEL)",
    ok: () => {
      const path =
        process.env.VT_UVR_MODEL ??
        `${process.env.LOCALAPPDATA ?? ""}/videotrans/models/UVR-MDX-NET-Inst_HQ_4.onnx`;
      return existsSync(path) && statSync(path).isFile();
    },
    hint: "背景分离场景用（缺 → 分离场景跳过）",
    optional: true,
  },
  {
    name: "测试音频 VT_TEST_AUDIO",
    ok: () => !!process.env.VT_TEST_AUDIO && existsSync(process.env.VT_TEST_AUDIO),
    hint: "whisper_existing_audio 用例专用（通常不设置）",
    optional: true,
  },
];

let missing = 0;
console.log("== 真机回归环境预检 ==");
for (const check of checks) {
  const pass = check.ok();
  if (!pass) missing++;
  console.log(`  [${pass ? "OK" : "MISS"}] ${check.name}`);
  if (!pass) console.log(`         ${check.hint}`);
}
if (missing > 0) {
  console.log(`\n⚠ ${missing} 项缺失：对应用例会跳过（不视为失败）。`);
} else {
  console.log("\n✔ 环境齐备，全部真机用例都会实际执行。");
}

console.log("\n== 运行真机测试 (cargo test --features inference -- --ignored) ==");
const result = spawnSync(
  "cargo",
  ["test", "--features", "inference", "--", "--ignored", "--nocapture"],
  { cwd: join(root, "src-tauri"), stdio: "inherit", shell: process.platform === "win32" },
);

console.log("\n== 汇总 ==");
if (result.status === 0) {
  console.log("✔ 全部真机用例通过（或被预期跳过）。");
} else {
  console.log(`✘ 真机回归失败（exit=${result.status}）。请查看上方失败用例输出；`);
  console.log(
    "  测试进程的日志写临时目录，不污染用户日志。",
  );
}
process.exit(result.status ?? 1);