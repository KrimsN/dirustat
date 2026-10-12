//! L0: быстрый разбор NTFS MFT (путь «как у WizTree»).
//!
//! Статус: заглушка (веха M3). Здесь будет парсинг MFT-записей из сырого тома. Крейт — единственное
//! место workspace, где допустим `unsafe` (ADR 0002): только для Win32 FFI (`CreateFileW`,
//! `DeviceIoControl`, чтение тома). Парсинг записей — только safe-код, с fuzz-таргетом.

/// Результат проверки возможности быстрого пути.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MftSupport {
    /// Платформа или права не позволяют читать MFT; используем переносимый обход.
    Unavailable,
}

/// Проверяет, доступен ли быстрый путь для тома. Пока всегда `Unavailable`.
pub fn probe(_volume: &str) -> MftSupport {
    MftSupport::Unavailable
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_reports_unavailable() {
        assert_eq!(probe("C:"), MftSupport::Unavailable);
    }
}
