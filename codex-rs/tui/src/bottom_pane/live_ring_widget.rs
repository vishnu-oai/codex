use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::WidgetRef;

pub(crate) struct LiveRingWidget {
    max_rows: u16,
    rows: Vec<Line<'static>>,
}

impl LiveRingWidget {
    pub(crate) fn new() -> Self {
        Self { max_rows: 0, rows: Vec::new() }
    }

    pub(crate) fn set_max_rows(&mut self, max_rows: u16) {
        self.max_rows = max_rows;
    }

    pub(crate) fn set_rows(&mut self, rows: Vec<Line<'static>>) {
        self.rows = rows;
    }

    pub(crate) fn desired_height(&self, _width: u16) -> u16 {
        let len = self.rows.len() as u16;
        if self.max_rows == 0 { len } else { len.min(self.max_rows) }
    }
}

impl WidgetRef for &LiveRingWidget {
    fn render_ref(&self, area: Rect, buf: &mut Buffer) {
        let total = self.rows.len();
        let start = total.saturating_sub(self.desired_height(area.width) as usize);
        for (i, line) in self.rows[start..].iter().enumerate() {
            let y = area.y + i as u16;
            if y >= area.y + area.height { break; }
            line.render((area.x, y), buf);
        }
    }
}


