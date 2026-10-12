//! L2: `Session` и реестр команд. **Всё пользовательское поведение — команда** (ADR 0003).
//!
//! UI, CLI, control-канал и MCP вызывают команды по `id`. Команда: id, подпись, группа меню,
//! шорткат, описание параметров, `enabled`, `run`. Программный вызов никогда не открывает диалогов.
//!
//! Статус: каркас (веха M1).

use dirustat_model::Tree;
use dirustat_scan::empty_tree;
use dirustat_treemap::{Rect, squarify};
use std::fmt;

/// Ошибка выполнения команды. `run` обязан возвращать её, а не паниковать, на любых параметрах.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommandError {
    UnknownCommand(String),
    InvalidParams(String),
    Unsupported(String),
}

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownCommand(id) => write!(f, "unknown command: {id}"),
            Self::InvalidParams(why) => write!(f, "invalid params: {why}"),
            Self::Unsupported(what) => write!(f, "unsupported: {what}"),
        }
    }
}

impl std::error::Error for CommandError {}

/// Состояние сессии: текущее дерево и производные данные.
#[derive(Debug)]
pub struct Session {
    tree: Tree,
}

impl Session {
    pub fn new() -> Self {
        Self { tree: empty_tree("") }
    }

    pub fn tree(&self) -> &Tree {
        &self.tree
    }

    /// Раскладка потомков корня в `bounds` — минимальный пример связки model + treemap.
    pub fn layout_root(&self, bounds: Rect) -> Vec<Rect> {
        let mut weights: Vec<u64> =
            self.tree.children(self.tree.root()).filter_map(|id| self.tree.get(id).map(|n| n.total_size())).collect();
        weights.sort_unstable_by(|a, b| b.cmp(a));
        squarify(&weights, bounds)
    }

    /// Выполняет команду по id. Реестр команд появится в M1.
    pub fn run(&mut self, id: &str, _params: &str) -> Result<(), CommandError> {
        Err(CommandError::UnknownCommand(id.to_owned()))
    }
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_command_is_an_error_not_a_panic() {
        let mut s = Session::new();
        assert_eq!(s.run("nope", "{}"), Err(CommandError::UnknownCommand("nope".into())));
    }

    #[test]
    fn empty_session_lays_out_nothing() {
        let s = Session::new();
        assert!(s.layout_root(Rect { x: 0.0, y: 0.0, w: 100.0, h: 100.0 }).is_empty());
    }
}
