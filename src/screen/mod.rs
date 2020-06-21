use std::cell::RefCell;
use std::rc::Rc;
use std::ops::Deref;
use std::borrow::BorrowMut;
use std::io::*;
use std::process::{Command, Stdio, ChildStdin, ChildStdout, exit};
use std::thread;
use std::sync::Arc;
use crossbeam::queue::SegQueue;
use std::str;
use crossterm::event::{KeyCode, KeyModifiers};

use std::path::Path;
use nix::fcntl::{OFlag, open};
use nix::pty::{grantpt, posix_openpt, ptsname, unlockpt, Winsize};
use nix::sys::stat::Mode;
use nix::unistd::{fork, ForkResult, close, setsid, dup2, Pid};
use std::os::unix::io::{AsRawFd, FromRawFd, RawFd};
use nix::{ioctl_none_bad, ioctl_write_ptr_bad};

use libc;
use std::os::raw::{c_char, c_void};
use std::ffi::{CString, CStr};
use core::ptr;
use std::fs::File;
use libc::{TIOCSCTTY, TIOCSWINSZ};
use term::Attr;

ioctl_write_ptr_bad!(set_window_size, TIOCSWINSZ, Winsize);
ioctl_none_bad!(set_controlling_terminal, TIOCSCTTY);

#[derive(Copy, Clone)]
pub struct CharacterCell {
    pub ch: char,
    pub fg: u8,
    pub bg: u8,
    pub attrs: Attr,
}

enum State {
    Normal,
    // Waiting for any kind of byte
    ExpectingControlChar,
    // We just got an ESC and expect a control sequence
    Csi,
    // ESC [ Control Sequence Introducer
    /* Multiparameter sequences are of the form KIND P_1 ; P_2 ; ... TERMINATION */
    Csi1(Vec<u8>),
    // CSI with one buffer (CSI <num>) waiting for new buffer digits, a second buffer or a termination character
    Csi2(Vec<u8>, Vec<u8>),
    // CSI with two buffers (CSI <num> ; <num>) as above
    Csi3(Vec<u8>, Vec<u8>, Vec<u8>),
    // CSI with three buffers
    CsiQ(Vec<u8>),
    // CSI followed by '?'
    Osc1(Vec<u8>),
    // ESC ] Operating System Command
    Osc2(Vec<u8>, Vec<u8>),
}

struct EmbedGrid {
    cursor: (usize, usize),
    grid: Vec<CharacterCell>,
    state: State,
    fg_color: u8,
    bg_color: u8,
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
}

impl SimpleTerminalWindow {
    pub fn add_string(&mut self, s: String) {
        for c in s.bytes() {
            match (c, &mut self.grid.state) {
                (b'\x1b', State::Normal) => {
                    self.grid.state = State::ExpectingControlChar;
                }
                (b']', State::ExpectingControlChar) => {
                    let buf1 = Vec::new();
                    self.grid.state = State::Osc1(buf1);
                }
                (b'[', State::ExpectingControlChar) => {
                    self.grid.state = State::Csi;
                }
                (b'H', State::Csi) => {
                    self.grid.cursor = (0, 0);
                    self.grid.state = State::Normal;
                }

                (b'\r', State::Normal) => {
                    // carriage return x-> 0
                    self.grid.cursor.0 = 0;
                }
                (b'\n', State::Normal) => {
                    // newline y -> y + 1
                    if self.grid.cursor.1 + 1 < self.height as usize {
                        self.grid.cursor.1 += 1;
                    }
                }
                (0x08, State::Normal) => {
                    if self.grid.cursor.0 > 0 {
                        self.grid.cursor.0 -= 1;
                    }
                }
                (c, State::Normal) => {
                    self.grid.grid[self.grid.cursor.0 + self.grid.cursor.1 * self.width as usize].ch = c as char;
                    self.grid.grid[self.grid.cursor.0 + self.grid.cursor.1 * self.width as usize].fg = self.grid.fg_color;
                    self.grid.grid[self.grid.cursor.0 + self.grid.cursor.1 * self.width as usize].bg = self.grid.bg_color;
                    self.grid.cursor.0 += 1;
                }
                (b'H', State::Csi2(ref y, ref x)) => {
                    let orig_x = unsafe { std::str::from_utf8_unchecked(x) }
                        .parse::<usize>()
                        .unwrap_or(1);
                    let orig_y = unsafe { std::str::from_utf8_unchecked(y) }
                        .parse::<usize>()
                        .unwrap_or(1);

                    if orig_x - 1 <= self.width as usize && orig_y - 1 <= self.height as usize {
                        self.grid.cursor.0 = orig_x - 1;
                        self.grid.cursor.1 = orig_y - 1;
                    } else {
                        eprintln!(
                            "[error] terminal_size = {:?}, cursor = {:?} but cursor set to  [{},{}]",
                            (self.width, self.height), self.grid.cursor, orig_x, orig_y
                        );
                    }

                    self.grid.state = State::Normal;
                }
                (b'm', State::Csi1(ref buf1)) => {
                    match buf1.as_slice() {
                        b"30" => self.grid.fg_color = 0,
                        b"31" => self.grid.fg_color = 1,
                        b"32" => self.grid.fg_color = 2,
                        b"33" => self.grid.fg_color = 3,
                        b"34" => self.grid.fg_color = 4,
                        b"35" => self.grid.fg_color = 5,
                        b"36" => self.grid.fg_color = 6,
                        b"37" => self.grid.fg_color = 7,

                        b"39" => self.grid.fg_color = 8,
                        b"40" => self.grid.bg_color = 9,
                        b"41" => self.grid.bg_color = 10,
                        b"42" => self.grid.bg_color = 11,
                        b"43" => self.grid.bg_color = 12,
                        b"44" => self.grid.bg_color = 13,
                        b"45" => self.grid.bg_color = 15,
                        b"46" => self.grid.bg_color = 16,
                        b"47" => self.grid.bg_color = 17,

                        b"49" => self.grid.bg_color = 15,
                        _ => {}
                    }
                    self.grid.grid[self.grid.cursor.0 + self.grid.cursor.1 * self.width as usize].fg = self.grid.fg_color;
                    self.grid.grid[self.grid.cursor.0 + self.grid.cursor.1 * self.width as usize].bg = self.grid.bg_color;
                    self.grid.state = State::Normal;
                }
                (b'm', State::Csi3(ref buf1, ref buf2, ref buf3)) if buf1 == b"38" && buf2 == b"5" => {
                    /* ESC [ m 38 ; 5 ; fg_color_byte m */
                    /* Set only foreground color */
                    self.grid.fg_color = if let Ok(byte) =
                    u8::from_str_radix(unsafe { std::str::from_utf8_unchecked(buf3) }, 10)
                    {
                        byte
                    } else {
                        0
                    };
                    self.grid.grid[self.grid.cursor.0 + self.grid.cursor.1 * self.width as usize].fg = self.grid.fg_color;
                    self.grid.state = State::Normal;
                }
                (b'm', State::Csi3(ref buf1, ref buf2, ref buf3)) if buf1 == b"48" && buf2 == b"5" => {
                    /* ESC [ m 48 ; 5 ; fg_color_byte m */
                    /* Set only background color */
                    self.grid.bg_color = if let Ok(byte) =
                    u8::from_str_radix(unsafe { std::str::from_utf8_unchecked(buf3) }, 10)
                    {
                        byte
                    } else {
                        0
                    };
                    self.grid.grid[self.grid.cursor.0 + self.grid.cursor.1 * self.width as usize].bg = self.grid.bg_color;
                    self.grid.state = State::Normal;
                }
                (b'D', State::Csi1(buf)) => {
                    // ESC[{buf}D   CSI Cursor Backward {buf} Times
                    let offset = unsafe { std::str::from_utf8_unchecked(buf) }
                        .parse::<usize>()
                        .unwrap();
                    self.grid.cursor.0 = self.grid.cursor.0.saturating_sub(offset);
                    self.grid.state = State::Normal;
                }
                _ => {}
            }
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
    fn get_title(&self) -> Option<&str>;
    fn input(&mut self, input: String);
    fn set_size(&mut self, width: u16, height: u16);

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
        for i in 0..self.height {
            let slice = &self.grid.grid[((i * self.width) as usize)..(((i + 1) * self.width) as usize)];
            for c in slice {
                result.push(c.ch);
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

    fn get_title(&self) -> Option<&str> {
        return Some(&self.title);
    }

    fn input(&mut self, input: String) {
        self.add_string(input);
    }

    fn set_size(&mut self, width: u16, height: u16) {
        self.width = width;
        self.height = height;
        self.grid.grid = vec![CharacterCell {
            attrs: Attr::BackgroundColor(0),
            ch: ' ',
            bg: 0,
            fg: 15,
        }; width as usize * height as usize];

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
        if self.last_mouse_down_pos_coords.0 == self.x + self.last_size.0 &&
            self.last_mouse_down_pos_coords.1 >= self.last_pos.1 &&
            self.last_mouse_down_pos_coords.1 < self.last_pos.1 + self.last_size.1 + 1 {
            if x > self.x {
                self.set_size(x - self.x, self.height);
            }
        }
        if self.last_mouse_down_pos_coords.1 == self.y + self.last_size.1 &&
            self.last_mouse_down_pos_coords.0 >= self.last_pos.0 &&
            self.last_mouse_down_pos_coords.0 < self.last_pos.0 + self.last_size.0 + 1 {
            if y > self.y {
                self.set_size(self.width, y - self.y);
            }
        }

        if self.last_mouse_down_pos_coords.1 == self.last_pos.1 - 1 &&
            self.last_mouse_down_pos_coords.0 >= self.last_pos.0 &&
            self.last_mouse_down_pos_coords.0 < self.last_pos.0 + self.last_size.0 + 1 {
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
                self.master_fd.write((&[0x08 as u8, ' ' as u8, 0x08 as u8])).unwrap();
            }
            _ => {}
        }
    }

    fn is_touching(&self, x: u16, y: u16) -> bool {
        return x >= self.x - 1 && x <= self.x + self.width &&
            y >= self.y - 1 && y <= self.y + self.height;
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
            Ok(ForkResult::Parent { child, .. }) => {
                child
            }
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

                    if libc::execle(shell.as_ptr(), arg0.as_ptr(), ptr::null() as *const c_void, env.as_ptr()) == -1 {
                        println!("ERROR execle() ({})", errno::errno());
                    }

                    exit(-1);
                    panic!();
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
                bg_color: 0,
                fg_color: 15,
                grid: vec![CharacterCell {
                    attrs: Attr::BackgroundColor(0),
                    ch: ' ',
                    bg: 0,
                    fg: 15,
                }; width as usize * height as usize],
                state: State::Normal,
            },
            last_mouse_down_pos_coords: (0, 0),
            last_size: (width, height),
            last_pos: (x, y),
            master_fd: m,
            child_pid,
            queue,
        };
    }
}

pub struct Screen {
    pub containers: Vec<Rc<RefCell<Box<dyn Container>>>>
}

impl Screen {
    pub fn new() -> Screen {
        return Screen {
            containers: vec![]
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
            if self.containers.get(i).unwrap().deref().borrow().is_touching(x, y) {
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