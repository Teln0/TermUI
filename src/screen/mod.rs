use std::cell::RefCell;
use std::rc::Rc;
use std::ops::Deref;
use std::borrow::BorrowMut;
use std::io::*;
use std::process::{Command, Stdio, ChildStdin, ChildStdout};
use std::thread;
use std::sync::Arc;
use crossbeam::queue::SegQueue;
use std::str;
use crossterm::event::{KeyCode, KeyModifiers};

pub struct SimpleBufferWindow {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
    pub title: String,
    scroll_y: u16,
    lines: Vec<String>,
    last_mouse_down_pos_coords: (u16, u16),
    last_size: (u16, u16),
    last_pos: (u16, u16),
    stdin: ChildStdin,
    queue: Arc<SegQueue<String>>
}

impl SimpleBufferWindow {
    pub fn add_line(&mut self, line: String) {
        let mut i = 0;
        for l in line.split("\n") {
            if !l.eq("") || i > 0 {
                if i == 0 {
                    self.lines.last_mut().unwrap().push_str(l);
                }
                else {
                    self.lines.push(l.to_string());
                }
            }
            i+=1;
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

    fn on_scroll_y(&mut self, amount: i16);
    fn on_mouse_down(&mut self, x: u16, y: u16);
    fn on_mouse_up(&mut self, x: u16, y: u16);
    fn on_mouse_drag(&mut self, x: u16, y: u16);
    fn on_key(&mut self, code: KeyCode, modifiers: KeyModifiers);

    fn is_touching(&self, x: u16, y: u16) -> bool;
}

impl Container for SimpleBufferWindow {
    fn update_content(&mut self) {
        while self.queue.len() > 0 {
            let pop = self.queue.pop();
            if pop.is_ok() {
                self.add_line(pop.unwrap());
            }
        }
    }

    fn get_content(&self) -> String {
        let mut result = "".to_string();
        for i in self.scroll_y..(self.get_height() + self.scroll_y) {
            let current_line = self.lines.get(i as usize);
            if current_line.is_some() {
                let mut current_line_str = current_line.unwrap().clone();
                current_line_str.truncate(self.get_width() as usize);
                result += current_line_str.as_str();
                result += "\n";
            } else {
                return result;
            }
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
        self.add_line(input);
        /*
        if self.scroll_y as usize <= self.lines.len() {
            if self.lines.len() - self.scroll_y as usize > self.height as usize {
                self.scroll_y = (self.lines.len() - self.scroll_y as usize - self.height as usize) as u16;
            }
        }
         */
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
                self.width = x - self.x;
            }
        }
        if self.last_mouse_down_pos_coords.1 == self.y + self.last_size.1 &&
            self.last_mouse_down_pos_coords.0 >= self.last_pos.0 &&
            self.last_mouse_down_pos_coords.0 < self.last_pos.0 + self.last_size.0 + 1 {
            if y > self.y {
                self.height = y - self.y;
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
                self.stdin.write(&[c as u8]).unwrap();
                self.lines.last_mut().unwrap().push(c);
            }
            KeyCode::Enter => {
                self.stdin.write(&['\n' as u8]).unwrap();
                self.lines.push("".to_string());
            }
            KeyCode::Backspace => {
                self.stdin.write((&[0x08 as u8, ' ' as u8, 0x08 as u8])).unwrap();
                let len = self.lines.last().unwrap().len();
                if len > 0 {
                    self.lines.last_mut().unwrap().truncate(len - 1);
                }
            }
            _ => {}
        }
    }

    fn is_touching(&self, x: u16, y: u16) -> bool {
        return x >= self.x - 1 && x <= self.x + self.width &&
            y >= self.y - 1 && y <= self.y + self.height;
    }
}

impl SimpleBufferWindow {
    pub fn new(x: u16, y: u16, width: u16, height: u16, title: String) -> SimpleBufferWindow {
        let lines: Vec<String> = vec!["".to_string()];
        let mut shell = Command::new("bash")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut stdin = shell.stdin.take().unwrap();
        let queue: Arc<SegQueue<String>> = Arc::new(SegQueue::new());
        let q = queue.clone();
        let h = thread::spawn(move || {
            let mut shellout = shell.stdout.take().unwrap();
            loop {
                let mut buf = [0; 1024];
                let r = shellout.read(&mut buf);
                let n = r.unwrap();
                let str = str::from_utf8(&buf[..n]).unwrap().to_string();
                q.deref().push(str.clone());
            }
        });

        return SimpleBufferWindow {
            x,
            y,
            width,
            height,
            title,
            scroll_y: 0,
            lines,
            last_mouse_down_pos_coords: (0, 0),
            last_size: (width, height),
            last_pos: (x, y),
            stdin,
            queue
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