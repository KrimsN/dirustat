//! L3: control-канал (JSON по TCP на 127.0.0.1 с ключом) и MCP-сервер.
//!
//! Статус: каркас (веха M2). Протокол — `docs/control-protocol.md`. Любое действие, доступное
//! человеку в UI, должно быть доступно агенту через `engine.execute`; скриншоты окна — `ui.screenshot`.

use dirustat_engine::{CommandError, Session};

/// Версия протокола control-канала.
pub const PROTOCOL_VERSION: u32 = 1;

/// Заготовка обработчика: пока только выполняет команду в сессии.
pub fn execute(session: &mut Session, command: &str, params: &str) -> Result<(), CommandError> {
    session.run(command, params)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_command_surfaces_error() {
        let mut s = Session::new();
        assert!(execute(&mut s, "missing", "{}").is_err());
        assert_eq!(PROTOCOL_VERSION, 1);
    }
}
