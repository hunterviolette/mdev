#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod controller;
mod environment;

fn main() {
    controller::run();
}
