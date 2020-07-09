use crossbeam::queue::SegQueue;
use crossterm::event::{KeyCode, KeyModifiers};
use std::borrow::BorrowMut;
use std::cell::RefCell;
use std::io::*;
use std::ops::Deref;
use std::process::{exit, ChildStdin, ChildStdout, Command, Stdio};
use std::rc::Rc;
use std::str;
use std::sync::Arc;
use std::thread;

use nix::fcntl::{open, OFlag};
use nix::pty::{grantpt, posix_openpt, ptsname, unlockpt, Winsize};
use nix::sys::stat::Mode;
use nix::unistd::{close, dup2, fork, setsid, ForkResult, Pid};
use nix::{ioctl_none_bad, ioctl_write_ptr_bad};
use std::os::unix::io::{AsRawFd, FromRawFd, RawFd};
use std::path::Path;

use core::ptr;
use libc;
use libc::{TIOCSCTTY, TIOCSWINSZ};
use std::alloc::handle_alloc_error;
use std::ffi::{CStr, CString};
use std::fs::File;
use std::os::raw::{c_char, c_void};
use std::thread::current;
use term::Attr;
use vte::{Parser, Perform};

ioctl_write_ptr_bad!(set_window_size, TIOCSWINSZ, Winsize);
ioctl_none_bad!(set_controlling_terminal, TIOCSCTTY);

#[derive(Copy, Clone)]
pub struct CharacterCell {
    pub ch: char,
    pub fg: u8,
    pub bg: u8,
    pub attrs: Attr,
}

struct EmbedGrid {
    printed_chars: usize,
    cursor: (usize, usize),
    grid: Vec<CharacterCell>,
    fg_color: u8,
    bg_color: u8,
    width: usize,
    height: usize,
}

pub struct SimpleTerminalWindow {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
    pub title: String,
    scroll_y: u16,
    grid: EmbedGrid,
    last_mouse_down_pos_coords: (u16, u16),
    last_size: (u16, u16),
    last_pos: (u16, u16),
    master_fd: File,
    child_pid: Pid,
    queue: Arc<SegQueue<String>>,
    vte_parser: Parser,
}

impl Perform for EmbedGrid {
    fn print(&mut self, c: char) {
        self.printed_chars += 1;

        self.grid[self.cursor.0 + self.cursor.1 * self.width as usize].ch = c;
        self.grid[self.cursor.0 + self.cursor.1 * self.width as usize].fg = self.fg_color;
        self.grid[self.cursor.0 + self.cursor.1 * self.width as usize].bg = self.bg_color;

        self.cursor.0 += 1;
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            0x08 => {
                if self.cursor.0 > 0 {
                    self.cursor.0 -= 1;
                }
            }
            0x0A => {
                if self.cursor.1 <= self.height {
                    self.cursor.1 += 1;
                }
            }
            0x0D => {
                self.cursor.0 = 0;
            }
            0x07 => {
                print!("\x07");
            }
            c => {
                println!("      {:?}", c as char);
                panic!();
            }
        };
    }

    fn hook(&mut self, params: &[i64], intermediates: &[u8], ignore: bool, action: char) {
        panic!();
    }

    fn put(&mut self, byte: u8) {
        panic!();
    }

    fn unhook(&mut self) {
        panic!();
    }

    fn osc_dispatch(&mut self, params: &[&[u8]], bell_terminated: bool) {
        panic!();
    }

    fn csi_dispatch(&mut self, params: &[i64], intermediates: &[u8], ignore: bool, action: char) {
        return;
        match action {
            'A' => {
                // Cursor Up
                if (self.cursor.1 as i64) > params[0] {
                    self.cursor.1 -= params[0] as usize;
                } else {
                    self.cursor.1 = 0;
                }
            }
            'B' => {
                // Cursor Down
                if (self.height as i64) > self.cursor.1 as i64 + params[0] {
                    self.cursor.1 += params[0] as usize;
                } else {
                    self.cursor.1 = self.height - 1;
                }
            }
            'D' => {
                // Cursor Back
                if (self.cursor.0 as i64) > params[0] {
                    self.cursor.0 -= params[0] as usize;
                } else {
                    self.cursor.0 = 0;
                }
            }
            'C' => {
                // Cursor Forwards
                if (self.width as i64) > self.cursor.0 as i64 + params[0] {
                    self.cursor.0 += params[0] as usize;
                } else {
                    self.cursor.0 = self.height - 0;
                }
            }
            'E' => {
                // Go down then to the beginning of the line
                if (self.height as i64) > self.cursor.1 as i64 + params[0] {
                    self.cursor.1 += params[0] as usize;
                } else {
                    self.cursor.1 = self.height - 1;
                }
                self.cursor.0 = 0;
            }
            'F' => {
                // Go up then to the beginning of the line
                if (self.cursor.1 as i64) > params[0] {
                    self.cursor.1 -= params[0] as usize;
                } else {
                    self.cursor.1 = 0;
                }
                self.cursor.0 = 0;
            }
            'G' => {
                // Set cursor horizontal pos
                if params[0] < self.width as i64 {
                    if params[0] > 0 {
                        self.cursor.0 = params[0] as usize;
                    } else {
                        self.cursor.0 = 0;
                    }
                } else {
                    self.cursor.0 = self.width - 1;
                }
            }
            'H' => {
                // Set cursor pos
                let y = params[0] - 1;
                let x = params[1] - 1;

                if x <= self.width as i64 {
                    if x > 0 {
                        self.cursor.0 = x as usize;
                    } else {
                        self.cursor.0 = 0;
                    }
                } else {
                    self.cursor.0 = self.width - 1;
                }

                if y <= self.height as i64 {
                    if y > 0 {
                        self.cursor.1 = y as usize;
                    } else {
                        self.cursor.1 = 0;
                    }
                } else {
                    self.cursor.1 = self.height - 1;
                }
            }
            'J' => match params[0] {
                0 => {
                    let index = self.cursor.0 + self.cursor.1 * self.width;
                    let end = self.width * self.height;
                    for i in index..end {
                        self.grid[i] = CharacterCell {
                            fg: 37,
                            bg: 40,
                            ch: ' ',
                            attrs: Attr::BackgroundColor(40),
                        }
                    }
                }
                1 => {
                    let index = 0;
                    let end = self.cursor.0 + self.cursor.1 * self.width + 1;
                    for i in index..end {
                        self.grid[i] = CharacterCell {
                            fg: 37,
                            bg: 40,
                            ch: ' ',
                            attrs: Attr::BackgroundColor(40),
                        }
                    }
                }
                2 | 3 => {
                    let end = self.cursor.0 + self.cursor.1 * self.width + 1;
                    for i in 0..end {
                        self.grid[i] = CharacterCell {
                            fg: 37,
                            bg: 40,
                            ch: ' ',
                            attrs: Attr::BackgroundColor(40),
                        }
                    }
                }
                _ => {}
            },
            'm' => {
                // Select Graphic Rendition
                for i in params {
                    match i {
                        30 => {
                            // FG black
                            self.fg_color = 30;
                        }
                        31 => {
                            // FG red
                            self.fg_color = 31;
                        }
                        32 => {
                            // FG green
                            self.fg_color = 32;
                        }
                        33 => {
                            // FG yellow
                            self.fg_color = 33;
                        }
                        34 => {
                            // FG blue
                            self.fg_color = 34;
                        }
                        35 => {
                            // FG magenta
                            self.fg_color = 35;
                        }
                        36 => {
                            // FG cyan
                            self.fg_color = 36;
                        }
                        37 => {
                            // FG white
                            self.fg_color = 37;
                        }
                        39 => {
                            // Default FG color (white)
                            self.fg_color = 37;
                        }

                        40 => {
                            // FG black
                            self.bg_color = 40;
                        }
                        41 => {
                            // BG red
                            self.bg_color = 41;
                        }
                        42 => {
                            // BG green
                            self.bg_color = 42;
                        }
                        43 => {
                            // BG yellow
                            self.bg_color = 43;
                        }
                        44 => {
                            // BG blue
                            self.bg_color = 44;
                        }
                        45 => {
                            // BG magenta
                            self.bg_color = 45;
                        }
                        46 => {
                            // BG cyan
                            self.bg_color = 46;
                        }
                        47 => {
                            // BG white
                            self.bg_color = 47;
                        }
                        49 => {
                            // Default BG color (black)
                            self.bg_color = 40;
                        }

                        0 => {
                            // Reset all
                            self.fg_color = 37;
                            self.bg_color = 40;
                        }
                        _ => {}
                    }
                }
            }
            _ => {
                panic!();
            }
        }
    }

    fn esc_dispatch(&mut self, intermediates: &[u8], ignore: bool, byte: u8) {
        println!("                     {:?}                ", byte);
        panic!();
    }
}

impl SimpleTerminalWindow {
    pub fn add_string(&mut self, s: String) {
        for c in s.bytes() {
            self.vte_parser.advance(&mut self.grid, c);
        }
    }
}

pub trait Container {
    fn update_content(&mut self);
    fn get_content(&self) -> String;
    fn get_x(&self) -> u16;
    fn get_y(&self) -> u16;
    fn get_width(&self) -> u16;
    fn get_height(&self) -> u16;
    fn get_cursor(&self) -> (usize, usize);
    fn get_title(&self) -> Option<&str>;
    fn input(&mut self, input: String);
    fn set_size(&mut self, width: u16, height: u16);
    fn get_printed_chars(&self) -> usize;

    fn on_scroll_y(&mut self, amount: i16);
    fn on_mouse_down(&mut self, x: u16, y: u16);
    fn on_mouse_up(&mut self, x: u16, y: u16);
    fn on_mouse_drag(&mut self, x: u16, y: u16);
    fn on_key(&mut self, code: KeyCode, modifiers: KeyModifiers);

    fn is_touching(&self, x: u16, y: u16) -> bool;
}

impl Container for SimpleTerminalWindow {
    fn update_content(&mut self) {
        while self.queue.len() > 0 {
            let pop = self.queue.pop();
            if pop.is_ok() {
                self.add_string(pop.unwrap());
            }
        }
    }

    fn get_content(&self) -> String {
        let mut result = "".to_string();
        let mut prev_color_fg: u8 = self.grid.grid[0].fg;
        let mut prev_color_bg: u8 = self.grid.grid[0].bg;
        for i in 0..self.height {
            let slice =
                &self.grid.grid[((i * self.width) as usize)..(((i + 1) * self.width) as usize)];
            let mut x = 0;
            for c in slice {
                let foreground = c.fg;
                let mut background = c.bg;
                if x == self.grid.cursor.0 && i as usize == self.grid.cursor.1 {
                    background = 47;
                }
                if foreground != prev_color_fg {
                    result.push_str(format!("\x1B[{}m", foreground).as_str());
                    prev_color_fg = foreground;
                }
                if background != prev_color_bg {
                    result.push_str(format!("\x1B[{}m", background).as_str());
                    prev_color_bg = background;
                }

                result.push(c.ch);
                x += 1;
            }
            result.push('\n');
        }
        return result;
    }

    fn get_x(&self) -> u16 {
        return self.x;
    }

    fn get_y(&self) -> u16 {
        return self.y;
    }

    fn get_width(&self) -> u16 {
        return self.width;
    }

    fn get_height(&self) -> u16 {
        return self.height;
    }

    fn get_cursor(&self) -> (usize, usize) {
        return self.grid.cursor;
    }

    fn get_title(&self) -> Option<&str> {
        return Some(&self.title);
    }

    fn input(&mut self, input: String) {
        self.add_string(input);
    }

    fn set_size(&mut self, width: u16, height: u16) {
        self.width = width;
        self.height = height;
        self.grid.width = width as usize;
        self.grid.height = height as usize;
        self.grid.cursor = (0, 0);

        self.grid.grid = vec![
            CharacterCell {
                attrs: Attr::BackgroundColor(40),
                ch: ' ',
                bg: 40,
                fg: 37,
            };
            width as usize * height as usize
        ];
        let winsize = Winsize {
            ws_row: self.height,
            ws_col: self.width,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        let master_fd = self.master_fd.as_raw_fd();

        unsafe { set_window_size(master_fd, &winsize).unwrap() };
        nix::sys::signal::kill(self.child_pid, nix::sys::signal::SIGWINCH).unwrap();
    }

    fn get_printed_chars(&self) -> usize {
        return self.grid.printed_chars;
    }

    fn on_scroll_y(&mut self, amount: i16) {
        if amount < 0 {
            if (-amount) as u16 > self.scroll_y {
                self.scroll_y = 0;
            } else {
                self.scroll_y -= (-amount) as u16;
            }
        } else {
            self.scroll_y += amount as u16;
        }
    }

    fn on_mouse_down(&mut self, x: u16, y: u16) {
        self.last_mouse_down_pos_coords = (x, y);
        self.last_size = (self.width, self.height);
        self.last_pos = (self.x, self.y);
    }

    fn on_mouse_up(&mut self, x: u16, y: u16) {
        self.last_size = (self.width, self.height);
        self.last_pos = (self.x, self.y);
    }

    fn on_mouse_drag(&mut self, x: u16, y: u16) {
        if self.last_mouse_down_pos_coords.0 == self.x + self.last_size.0
            && self.last_mouse_down_pos_coords.1 >= self.last_pos.1
            && self.last_mouse_down_pos_coords.1 < self.last_pos.1 + self.last_size.1 + 1
        {
            if x > self.x {
                self.set_size(x - self.x, self.height);
            }
        }
        if self.last_mouse_down_pos_coords.1 == self.y + self.last_size.1
            && self.last_mouse_down_pos_coords.0 >= self.last_pos.0
            && self.last_mouse_down_pos_coords.0 < self.last_pos.0 + self.last_size.0 + 1
        {
            if y > self.y {
                self.set_size(self.width, y - self.y);
            }
        }

        if self.last_mouse_down_pos_coords.1 == self.last_pos.1 - 1
            && self.last_mouse_down_pos_coords.0 >= self.last_pos.0
            && self.last_mouse_down_pos_coords.0 < self.last_pos.0 + self.last_size.0 + 1
        {
            if x > (self.last_mouse_down_pos_coords.0 - self.last_pos.0) {
                self.x = x - (self.last_mouse_down_pos_coords.0 - self.last_pos.0);
            }

            if y > (self.last_mouse_down_pos_coords.1 - (self.last_pos.1 - 1)) {
                self.y = y - (self.last_mouse_down_pos_coords.1 - (self.last_pos.1 - 1));
            }
        }
    }

    fn on_key(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        match code {
            KeyCode::Char(c) => {
                self.master_fd.write(&[c as u8]).unwrap();
            }
            KeyCode::Enter => {
                self.master_fd.write(&['\n' as u8]).unwrap();
            }
            KeyCode::Backspace => {
                self.master_fd.write((&[0x08 as u8])).unwrap();
            }
            KeyCode::Left => {
                self.master_fd
                    .write_all(&[0x1B as u8, '[' as u8, 'D' as u8]);
            }
            KeyCode::Right => {
                self.master_fd
                    .write_all(&[0x1B as u8, '[' as u8, 'C' as u8]);
            }
            KeyCode::Up => {
                self.master_fd
                    .write_all(&[0x1B as u8, '[' as u8, 'A' as u8]);
            }
            KeyCode::Down => {
                self.master_fd
                    .write_all(&[0x1B as u8, '[' as u8, 'B' as u8]);
            }
            _ => {}
        }
    }

    fn is_touching(&self, x: u16, y: u16) -> bool {
        return x >= self.x - 1
            && x <= self.x + self.width
            && y >= self.y - 1
            && y <= self.y + self.height;
    }
}

impl SimpleTerminalWindow {
    pub fn new(x: u16, y: u16, width: u16, height: u16, title: String) -> SimpleTerminalWindow {
        let lines: Vec<String> = vec!["".to_string()];

        let queue: Arc<SegQueue<String>> = Arc::new(SegQueue::new());
        let q = queue.clone();

        let master_fd = posix_openpt(OFlag::O_RDWR).unwrap();
        grantpt(&master_fd).unwrap();
        unlockpt(&master_fd).unwrap();
        let slave_name = unsafe { ptsname(&master_fd) }.unwrap();
        let slave_fd = open(Path::new(&slave_name), OFlag::O_RDWR, Mode::empty()).unwrap();
        let mut m: File = unsafe { std::fs::File::from_raw_fd(master_fd.as_raw_fd()) };
        let child_pid;

        child_pid = match fork() {
            Ok(ForkResult::Parent { child, .. }) => child,
            Ok(ForkResult::Child) => {
                setsid().unwrap();
                unsafe {
                    if libc::ioctl(slave_fd, TIOCSCTTY) == -1 {
                        println!("ERROR ioctl() {}", errno::errno());
                    }
                }
                dup2(slave_fd, 0); // stdin
                dup2(slave_fd, 1); // stdout
                dup2(slave_fd, 2); // stderr
                unsafe {
                    let shell = CString::new("/bin/bash").unwrap();
                    let arg0 = shell.clone();
                    let term = CString::new("TERM=dumb").unwrap();
                    let env = [term.as_ptr(), ptr::null()];

                    if libc::execle(
                        shell.as_ptr(),
                        arg0.as_ptr(),
                        ptr::null() as *const c_void,
                        env.as_ptr(),
                    ) == -1
                    {
                        println!("ERROR execle() ({})", errno::errno());
                    }

                    exit(-1);
                }
            }
            Err(e) => panic!(e),
        };

        std::thread::Builder::new()
            .spawn(move || {
                let winsize = Winsize {
                    ws_row: height,
                    ws_col: width,
                    ws_xpixel: 0,
                    ws_ypixel: 0,
                };
                let master_fd = master_fd.as_raw_fd();
                unsafe { set_window_size(master_fd, &winsize).unwrap() };
                let mut master_file: File = unsafe { std::fs::File::from_raw_fd(master_fd) };
                fn liaison(mut pty_fd: std::fs::File, q: Arc<SegQueue<String>>) {
                    loop {
                        let mut buf = [0; 1024];
                        let r = pty_fd.read(&mut buf);
                        let n = r.unwrap();
                        let str = str::from_utf8(&buf[..n]).unwrap().to_string();
                        q.push(str);
                    }
                }

                liaison(master_file, q);
            })
            .unwrap();

        return SimpleTerminalWindow {
            x,
            y,
            width,
            height,
            title,
            scroll_y: 0,
            grid: EmbedGrid {
                cursor: (0, 0),
                width: width as usize,
                height: height as usize,
                bg_color: 0,
                fg_color: 15,
                grid: vec![
                    CharacterCell {
                        attrs: Attr::BackgroundColor(0),
                        ch: ' ',
                        bg: 40,
                        fg: 37,
                    };
                    width as usize * height as usize
                ],
                printed_chars: 0,
            },
            last_mouse_down_pos_coords: (0, 0),
            last_size: (width, height),
            last_pos: (x, y),
            master_fd: m,
            child_pid,
            queue,
            vte_parser: Parser::new(),
        };
    }
}

pub struct Screen {
    pub containers: Vec<Rc<RefCell<Box<dyn Container>>>>,
    pub dev_console: Vec<String>,
}

impl Screen {
    pub fn new() -> Screen {
        return Screen {
            containers: vec![],
            dev_console: vec![],
        };
    }

    pub fn add_container(&mut self, con: Rc<RefCell<Box<dyn Container>>>) {
        self.containers.push(con);
    }

    pub fn get_container(&self, index: u16) -> Option<Rc<RefCell<Box<dyn Container>>>> {
        let con = self.containers.get(index as usize);
        if con.is_none() {
            return None;
        }
        return Some(con.unwrap().clone());
    }

    pub fn get_top_container(&self) -> Option<Rc<RefCell<Box<dyn Container>>>> {
        let con = self.containers.last();
        if con.is_none() {
            return None;
        }
        return Some(con.unwrap().clone());
    }

    pub fn check_top_container(&mut self, x: u16, y: u16) {
        let containers_len = self.containers.len();
        for i in (0..containers_len).rev() {
            if self
                .containers
                .get(i)
                .unwrap()
                .deref()
                .borrow()
                .is_touching(x, y)
            {
                let con;
                {
                    con = self.containers.remove(i);
                }
                self.containers.push(con);
                return;
            }
        }
    }
}
