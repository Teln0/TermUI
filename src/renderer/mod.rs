use std::io::{stdout, Write, BufWriter, Stdout, StdoutLock};
use crossterm::{
    ExecutableCommand, QueueableCommand,
    terminal, cursor, style::{self, Colorize}, Result
};
use crate::screen::Screen;
use std::ops::Deref;
use std::cmp::min;
use std::ffi::CString;

fn draw_rect(stdout: &mut Vec<u8>, x: u16, y: u16, w: u16, h: u16, max_w: u16, max_h: u16) {
    for x_iterator in x..(x+w) {
        for y_iterator in y..(y+h) {
            if (x_iterator < max_w && y_iterator < max_h) && ((x_iterator == x || y_iterator == y) || (x_iterator == x + w - 1 || y_iterator == y + h - 1)) {
                stdout
                    .queue(cursor::MoveTo(x_iterator,y_iterator)).unwrap()
                    .queue(style::PrintStyledContent( "█".white())).unwrap();
            }
            else {
                stdout
                    .queue(cursor::MoveTo(x_iterator,y_iterator)).unwrap()
                    .queue(style::PrintStyledContent( "█".black())).unwrap();
            }
        }
    }
}

fn render_text(stdout: &mut Vec<u8>, x: u16, y: u16, max_w: u16, max_h: u16, text: String) {
    let lines: Vec<&str> = text.split("\n").collect();
    let mut i = 0;
    for mut line in lines {
        if i > max_h {
            return;
        }
        let mut line_str;
        if line.len() > max_w as usize {
            line_str = line.to_string();
            line_str.truncate(max_w as usize);
        }
        else {
            line_str = line.to_string();
        }

        stdout
            .queue(cursor::MoveTo(x, y + i)).unwrap()
            .queue(crossterm::style::Print(line_str.as_str())).unwrap();

        i += 1;
    }
}

pub fn redraw(stdout: &mut Stdout, w: u16, h: u16, screen: &Screen) -> Result<()> {
    let mut vec: Vec<u8> = vec![];
    let mut s = stdout;
    let mut stdout = vec;

    // Clear the screen
    draw_rect(&mut stdout, 0, 0, w, h, w, h);

    for con in screen.containers.iter() {
        let con = con.deref().borrow();
        // Draw the container border around the window
        draw_rect(&mut stdout, con.get_x() - 1, con.get_y() - 1, con.get_width() + 2, con.get_height() + 2, w, h);
        // Draw the container title
        let title = con.get_title();
        if title.is_some() {
            stdout
                .queue(cursor::MoveTo(con.get_x(), con.get_y() - 1)).unwrap()
                .queue(crossterm::style::Print(con.get_title().unwrap()));
        }
        // Draw the container's content
        let mut con_width = con.get_width();
        let mut con_height = con.get_height();
        let mut do_render_text = true;
        if con.get_x() > w || con.get_y() > h - 1 {
            do_render_text = false;
        }
        if do_render_text {
            if con.get_x() + con_width > w {
                con_width -= con.get_x() + con_width - w;
            }
            if con.get_y() + con_height > h {
                con_height -= con.get_y() + con_height - h;
            }
            render_text(&mut stdout, con.get_x(), con.get_y(), con_width, con_height - 1, con.get_content());
        }
        // Draw the resize handles
        if con.get_x() + con.get_width() < w {
            stdout
                .queue(cursor::MoveTo(con.get_x() + con.get_width(), con.get_y() + con.get_height() / 2)).unwrap()
                .queue(crossterm::style::Print("↔"))?;
        }
        if con.get_y() + con.get_height() < h {
            stdout
                .queue(cursor::MoveTo(con.get_x() + con.get_width() / 2, con.get_y() + con.get_height())).unwrap()
                .queue(crossterm::style::Print("↕"))?;
        }
    }

    // Render some info about TermUI
    let info_string = format!("Stdout buffer size : {}", stdout.len());
    stdout
        .queue(cursor::MoveTo(2, 0)).unwrap()
        .queue(crossterm::style::Print(info_string));

    s.write_all(&stdout);
    Ok(())
}