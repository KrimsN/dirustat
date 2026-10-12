//! L4: egui/eframe-оболочка. Тонкая: состояние читает из `Session`, действия отправляет командами.
//! Цвета и радиусы — только из `theme::Tokens`, не хардкодом. Ничего ниже L4 не знает про egui.
//!
//! Статус: каркас (веха M1 подключит egui/eframe/wgpu).

use dirustat_control::PROTOCOL_VERSION;
use dirustat_engine::Session;

/// Заголовок окна: имя продукта пишется `diRUSTat`.
pub const WINDOW_TITLE: &str = "diRUSTat";

/// Строка-баннер для приложений.
pub fn banner(session: &Session) -> String {
    format!("{WINDOW_TITLE} (control protocol v{PROTOCOL_VERSION}, {} nodes)", session.tree().len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn banner_mentions_product_name() {
        assert!(banner(&Session::new()).starts_with("diRUSTat"));
    }
}
