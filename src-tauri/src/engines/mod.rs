//! 裸引擎实现命名空间：STT / 翻译 / TTS 的最小推理或 IO 封装。
//!
//! 这里是「引擎」不是 port 实现——资源成本声明与 stage 包装在 adapters/ 注入，
//! port 语义在 ports/。依赖方向：engines 不依赖 ports；adapters 依赖 ports + engines。

pub mod stt;
pub mod translate;
pub mod tts;
