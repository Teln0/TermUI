mod renderer;
mod screen;

use crate::screen::{Screen, SimpleTerminalWindow};
use crossterm::cursor::{DisableBlinking, EnableBlinking};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use crossterm::{event::*, terminal::size, QueueableCommand, Result};
use std::borrow::Borrow;
use std::cell::RefCell;
use std::io::{stdout, Read, Write};
use std::ops::Deref;
use std::process::Stdio;
use std::process::{exit, Command};
use std::rc::Rc;
use std::thread::sleep;
use std::time::Duration;

fn main() {
    let mut stdout = std::io::stdout();
    let mut screen = Screen::new();

    enable_raw_mode();
    stdout
        .queue(DisableBlinking)
        .unwrap()
        .queue(EnableMouseCapture)
        .unwrap()
        .flush()
        .unwrap();

    screen.add_container(Rc::new(RefCell::new(Box::new(SimpleTerminalWindow::new(
        5,
        5,
        60,
        15,
        "1".to_string(),
    )))));

    screen.add_container(Rc::new(RefCell::new(Box::new(SimpleTerminalWindow::new(
        15,
        15,
        60,
        15,
        "2".to_string(),
    )))));

    screen.add_container(Rc::new(RefCell::new(Box::new(SimpleTerminalWindow::new(
        25,
        25,
        60,
        15,
        "3".to_string(),
    )))));

    let size = size();
    let mut current_w = 0;
    let mut current_h = 0;

    if size.is_ok() {
        let size = size.unwrap();
        current_w = size.0;
        current_h = size.1;
        renderer::redraw(&mut stdout, current_w, current_h, &screen);
    }
    loop {
        for con in screen.containers.iter() {
            con.deref().borrow_mut().update_content();
        }
        renderer::redraw(&mut stdout, current_w, current_h, &screen);
        while poll(Duration::from_millis(0)).unwrap() {
            let event = read();
            if event.is_ok() {
                match event.unwrap() {
                    Event::Resize(w, h) => {
                        current_w = w;
                        current_h = h;
                    }
                    Event::Mouse(mouseEvent) => {
                        match mouseEvent {
                            MouseEvent::Down(mouse_button, x, y, key_modifiers) => {
                                screen.check_top_container(x, y);
                                screen
                                    .get_top_container()
                                    .unwrap()
                                    .borrow_mut()
                                    .on_mouse_down(x, y);
                            }
                            MouseEvent::Up(mouse_button, x, y, key_modifiers) => {
                                screen
                                    .get_top_container()
                                    .unwrap()
                                    .borrow_mut()
                                    .on_mouse_up(x, y);
                            }
                            MouseEvent::Drag(mouse_button, x, y, key_modifiers) => {
                                screen
                                    .get_top_container()
                                    .unwrap()
                                    .borrow_mut()
                                    .on_mouse_drag(x, y);
                            }
                            MouseEvent::ScrollUp(x, y, key_modifiers) => {
                                screen
                                    .get_top_container()
                                    .unwrap()
                                    .borrow_mut()
                                    .on_scroll_y(-1);
                            }
                            MouseEvent::ScrollDown(x, y, key_modifiers) => {
                                screen
                                    .get_top_container()
                                    .unwrap()
                                    .borrow_mut()
                                    .on_scroll_y(1);
                            }
                            _ => {}
                        };
                    }
                    Event::Key(key_event) => {
                        if key_event.code == KeyCode::Char('c') {
                            if key_event.modifiers == KeyModifiers::CONTROL {
                                stdout
                                    .queue(EnableBlinking)
                                    .unwrap()
                                    .queue(DisableMouseCapture)
                                    .unwrap();
                                disable_raw_mode().unwrap();
                            }
                        }

                        screen
                            .get_top_container()
                            .unwrap()
                            .borrow_mut()
                            .on_key(key_event.code, key_event.modifiers);
                    }
                    _ => {}
                }
            }
        }
    }
}
