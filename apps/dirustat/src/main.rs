//! Десктопное приложение. Пока печатает баннер; окно eframe появится в M1.
#![allow(clippy::print_stdout)]

use dirustat_engine::Session;

fn main() {
    println!("{}", dirustat_ui::banner(&Session::new()));
}
