//! Общие типы для всех процессов шелла: IPC-конверт, топики, версия протокола.
//!
//! Инвариант INV4 (`docs/architecture/0001-foundations.md`): всё IPC — schema-first,
//! типы живут здесь и версионируются.
#![deny(missing_docs)]

use serde::{Deserialize, Serialize};

/// Версия протокола. Инкрементируется при несовместимом изменении конверта.
pub const PROTO_VERSION: u32 = 0;

/// Монотонный номер состояния/события, который ведёт нормализатор композитора.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Seq(pub u64);

/// Топик подписки: режим `snapshot + delta` (эталон — поток событий Niri).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Topic {
    /// Воркспейсы композитора.
    CompositorWorkspaces,
    /// Окна композитора.
    CompositorWindows,
    /// Выходы (мониторы).
    CompositorOutputs,
    /// Нотификации и история.
    Notifications,
    /// Системный трей.
    Tray,
    /// Аудио-устройства и потоки.
    Audio,
    /// Сеть.
    Network,
    /// Bluetooth.
    Bluetooth,
    /// Питание и батарея.
    Power,
    /// Медиаплееры (MPRIS).
    Mpris,
    /// История клипборда.
    Clipboard,
    /// Тема и палитра.
    Theme,
    /// Конфиг и слои.
    Config,
    /// Плагины.
    Plugins,
    /// AI-подсистема.
    Ai,
}

/// Обёртка над топиком для подписки/отписки.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Subscribe {
    /// Топик.
    pub topic: Topic,
    /// Запросить полный снапшот первым сообщением.
    pub snapshot: bool,
}

/// Входящий вызов (JSON-RPC 2.0).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcRequest {
    /// jsonrpc = "2.0".
    pub jsonrpc: String,
    /// Идентификатор запроса.
    pub id: u64,
    /// Метод, например `config.get`.
    pub method: String,
    /// Параметры.
    pub params: serde_json::Value,
}

impl RpcRequest {
    /// Создать запрос.
    #[must_use]
    pub fn new(id: u64, method: impl Into<String>, params: serde_json::Value) -> Self {
        Self {
            jsonrpc: "2.0".to_owned(),
            id,
            method: method.into(),
            params,
        }
    }
}

/// Ответ (JSON-RPC 2.0).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcResponse {
    /// jsonrpc = "2.0".
    pub jsonrpc: String,
    /// Идентификатор запроса.
    pub id: u64,
    /// Результат.
    pub result: Option<serde_json::Value>,
    /// Ошибка.
    pub error: Option<RpcError>,
}

/// Ошибка RPC.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcError {
    /// Код (JSON-RPC или наш).
    pub code: i32,
    /// Сообщение.
    pub message: String,
}
