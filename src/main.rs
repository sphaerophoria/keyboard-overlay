use std::{
    collections::VecDeque,
    fs::File,
    io::Read,
    mem::MaybeUninit,
    path::PathBuf,
    sync::mpsc::{self, Receiver, Sender},
    thread,
};

use eframe::egui;
use egui::{FontFamily, RichText};

mod input_bindings;

// https://docs.kernel.org/input/input.html
// value is the value the event carries. Either a relative change for EV_REL, absolute
// new value for EV_ABS (joysticks ...), or 0 for EV_KEY for release, 1 for keypress
// and 2 for autorepeat
#[allow(unused)]
#[derive(Eq, PartialEq)]
enum KeyPressState {
    Up = 0,
    Down = 1,
}

#[derive(Debug)]
pub enum KeyPress {
    Ctrl,
    Alt,
    Shift,
    Super,
    Other(String),
}

impl KeyPress {
    fn from_code(code: u16) -> KeyPress {
        use input_bindings::*;
        match code as u32 {
            KEY_LEFTCTRL | KEY_RIGHTCTRL => KeyPress::Ctrl,
            KEY_LEFTSHIFT | KEY_RIGHTSHIFT => KeyPress::Shift,
            KEY_LEFTALT | KEY_RIGHTALT => KeyPress::Alt,
            KEY_LEFTMETA | KEY_RIGHTMETA => KeyPress::Super,
            _ => KeyPress::Other(code_to_str(code).to_string()),
        }
    }
}

#[derive(Debug)]
enum ArgParseError {
    EventInputMissing,
}

struct Args {
    event_input_path: PathBuf,
}

impl Args {
    fn try_parse<It: Iterator<Item = String>>(mut arg_it: It) -> Result<Args, ArgParseError> {
        // Skip program name
        let _ = arg_it.next();

        let mut event_input_path = None;

        while let Some(arg) = arg_it.next() {
            match arg.as_str() {
                "--event-input-path" => {
                    event_input_path = arg_it.next().map(Into::into);
                }
                "--help" => {
                    println!("{}", Args::help());
                    std::process::exit(1);
                }
                s => {
                    println!("Invalid argument: {s}");
                    println!("{}", Args::help());
                    std::process::exit(1);
                }
            }
        }

        let event_input_path = event_input_path.ok_or(ArgParseError::EventInputMissing)?;

        Ok(Args { event_input_path })
    }

    fn parse<It: Iterator<Item = String>>(arg_it: It) -> Args {
        match Self::try_parse(arg_it) {
            Ok(v) => v,
            Err(e) => {
                println!("Argument parsing failed: {e:?}");
                println!("{}", Args::help());
                std::process::exit(1);
            }
        }
    }

    fn help() -> String {
        "\n\
            keyboard-overlay: Displays keys in an overlay\n\
\n\
            Args:\n\
            --event-input-path [path]: Path to read keyboard events from\n\
            --help: Show this help and exit\n\
        "
        .to_string()
    }
}

struct InputEvent {
    event: input_bindings::input_event,
}

fn reader_thread(tx: Sender<InputEvent>, rx: Receiver<egui::Context>, event_input_path: PathBuf) {
    let ctx = rx.recv().unwrap();

    let mut f = File::open(event_input_path).unwrap();

    unsafe {
        loop {
            let mut event = MaybeUninit::<input_bindings::input_event>::uninit();
            {
                let event_buf = std::slice::from_raw_parts_mut(
                    event.as_mut_ptr() as *mut u8,
                    core::mem::size_of::<input_bindings::input_event>(),
                );
                f.read_exact(event_buf).unwrap();
            }

            let event = event.assume_init();

            // FIXME: Ioctl to filter on read
            // from input-event-codes.h
            const EV_KEY: u16 = 1;

            if event.type_ != EV_KEY {
                continue;
            }

            let event = InputEvent { event };

            tx.send(event).unwrap();
            ctx.request_repaint();
        }
    }
}

fn main() {
    let args = Args::parse(std::env::args());

    let (keycode_tx, keycode_rx) = mpsc::channel();
    let (context_tx, context_rx) = mpsc::channel();
    let _t = thread::spawn(move || reader_thread(keycode_tx, context_rx, args.event_input_path));

    let mut native_options = eframe::NativeOptions::default();
    native_options.viewport = native_options
        .viewport
        .with_transparent(true)
        .with_decorations(false)
        .with_always_on_top()
        .with_mouse_passthrough(true);

    eframe::run_native(
        "My egui App",
        native_options,
        Box::new(move |cc| Box::new(App::new(cc, keycode_rx, context_tx))),
    )
    .expect("Failed to run gui");
}

// Last keypress (plus modifier state)
// Number of times pressed
// When it was pressed

#[derive(Clone, Eq, PartialEq)]
struct Modifiers {
    ctrl: bool,
    shift: bool,
    alt: bool,
    sup: bool,
}

impl Modifiers {
    fn update(&mut self, key_press: &KeyPress, press_state: &KeyPressState) {
        match key_press {
            KeyPress::Alt => {
                self.alt = is_keydown(press_state);
            }
            KeyPress::Ctrl => {
                self.ctrl = is_keydown(press_state);
            }
            KeyPress::Shift => {
                self.shift = is_keydown(press_state);
            }
            KeyPress::Super => {
                self.sup = is_keydown(press_state);
            }
            _ => (),
        };
    }
}

struct KeyHistoryItem {
    key_s: String,
    modifiers: Modifiers,
}

struct App {
    rx: Receiver<InputEvent>,
    pressed_keycodes: VecDeque<KeyHistoryItem>,
    rendered_keycodes: Vec<String>,
    current_modifier_state: Modifiers,
}

impl App {
    fn new(
        cc: &eframe::CreationContext<'_>,
        rx: Receiver<InputEvent>,
        tx: Sender<egui::Context>,
    ) -> Self {
        tx.send(cc.egui_ctx.clone()).unwrap();
        cc.egui_ctx
            .style_mut(|style| style.visuals.window_fill = egui::Color32::TRANSPARENT);
        cc.egui_ctx.style_mut(|style| {
            style.visuals.panel_fill = egui::Color32::from_rgba_premultiplied(0, 0, 0, 127)
        });

        App {
            rx,
            pressed_keycodes: VecDeque::new(),
            rendered_keycodes: Vec::new(),
            current_modifier_state: Modifiers {
                ctrl: false,
                shift: false,
                alt: false,
                sup: false,
            },
        }
    }

    fn process_input_event(&mut self, event: &InputEvent) {
        let press_state = match event_press_state(event) {
            Some(v) => v,
            None => return,
        };

        let keypress = KeyPress::from_code(event.event.code);

        self.current_modifier_state.update(&keypress, &press_state);

        let key_s = match keypress {
            KeyPress::Other(s) => {
                if !is_keydown(&press_state) {
                    return;
                }
                s
            }
            _ => return,
        };

        // From this point on we know it is a key down of a non-modifier key

        let key_press_event = KeyHistoryItem {
            key_s,
            modifiers: self.current_modifier_state.clone(),
        };

        self.pressed_keycodes.push_back(key_press_event);
        let (rendered_keycodes, last_used_elem) =
            render_keycodes(self.pressed_keycodes.iter().rev());

        self.rendered_keycodes = rendered_keycodes;

        for _ in last_used_elem..self.pressed_keycodes.len() - 1 {
            self.pressed_keycodes.pop_front();
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        while let Ok(event) = self.rx.try_recv() {
            self.process_input_event(&event);
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.with_layout(egui::Layout::bottom_up(egui::Align::default()), |ui| {
                let item_it = self.rendered_keycodes.iter();
                for item in item_it {
                    let label_text = RichText::new(item)
                        .family(FontFamily::Monospace)
                        .color(egui::Color32::WHITE)
                        .size(15.0);

                    ui.label(label_text);
                }
            });
        });
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }
}

fn is_same_key_chord(a: &KeyHistoryItem, b: &KeyHistoryItem) -> bool {
    a.key_s == b.key_s && a.modifiers == b.modifiers
}

fn render_item(item: &KeyHistoryItem, count: &usize) -> String {
    let count_str = if *count > 1 {
        format!("x{}", count)
    } else {
        "".to_string()
    };

    let mut modifier_str = String::new();
    if item.modifiers.alt {
        modifier_str.push_str("Alt + ");
    }
    if item.modifiers.sup {
        modifier_str.push_str("Super + ");
    }
    if item.modifiers.ctrl {
        modifier_str.push_str("Ctrl + ");
    }
    if item.modifiers.shift {
        modifier_str.push_str("Shift + ");
    }

    format!("{}{} {}", modifier_str, item.key_s, count_str)
}

fn event_press_state(event: &InputEvent) -> Option<KeyPressState> {
    const UP: i32 = KeyPressState::Up as i32;
    const DOWN: i32 = KeyPressState::Down as i32;
    match event.event.value {
        UP => Some(KeyPressState::Up),
        DOWN => Some(KeyPressState::Down),
        _ => None,
    }
}

fn is_keydown(press_state: &KeyPressState) -> bool {
    *press_state == KeyPressState::Down
}

fn render_keycodes<'a, It: Iterator<Item = &'a KeyHistoryItem>>(
    key_history: It,
) -> (Vec<String>, usize) {
    let mut key_history = key_history.enumerate();
    let mut ret = Vec::new();

    let mut last_item = match key_history.next() {
        Some((_, v)) => v,
        None => return (ret, 0),
    };
    let mut last_item_count = 1;
    let mut last_elem_idx = 1;

    const MAX_LINES: usize = 40;
    for (i, item) in key_history {
        last_elem_idx = i;
        if ret.len() > MAX_LINES {
            return (ret, last_elem_idx);
        }

        if is_same_key_chord(item, last_item) {
            last_item_count += 1;
        } else {
            ret.push(render_item(last_item, &last_item_count));
            last_item_count = 1;
        }

        last_item = item;
    }

    ret.push(render_item(last_item, &last_item_count));

    (ret, last_elem_idx)
}

fn code_to_str(code: u16) -> &'static str {
    match code {
        1 => "ESC",
        2 => "1",
        3 => "2",
        4 => "3",
        5 => "4",
        6 => "5",
        7 => "6",
        8 => "7",
        9 => "8",
        10 => "9",
        11 => "0",
        12 => "-",
        13 => "=",
        14 => "BACKSPACE",
        15 => "TAB",
        16 => "Q",
        17 => "W",
        18 => "E",
        19 => "R",
        20 => "T",
        21 => "Y",
        22 => "U",
        23 => "I",
        24 => "O",
        25 => "P",
        26 => "LEFTBRACE",
        27 => "RIGHTBRACE",
        28 => "ENTER",
        29 => "LEFTCTRL",
        30 => "A",
        31 => "S",
        32 => "D",
        33 => "F",
        34 => "G",
        35 => "H",
        36 => "J",
        37 => "K",
        38 => "L",
        39 => "SEMICOLON",
        40 => "APOSTROPHE",
        41 => "GRAVE",
        42 => "LEFTSHIFT",
        43 => "BACKSLASH",
        44 => "Z",
        45 => "X",
        46 => "C",
        47 => "V",
        48 => "B",
        49 => "N",
        50 => "M",
        51 => "COMMA",
        52 => "DOT",
        53 => "SLASH",
        54 => "RIGHTSHIFT",
        55 => "KPASTERISK",
        56 => "LEFTALT",
        57 => "SPACE",
        58 => "CAPSLOCK",
        59 => "F1",
        60 => "F2",
        61 => "F3",
        62 => "F4",
        63 => "F5",
        64 => "F6",
        65 => "F7",
        66 => "F8",
        67 => "F9",
        68 => "F10",
        69 => "NUMLOCK",
        70 => "SCROLLLOCK",
        71 => "KP7",
        72 => "KP8",
        73 => "KP9",
        74 => "KPMINUS",
        75 => "KP4",
        76 => "KP5",
        77 => "KP6",
        78 => "KPPLUS",
        79 => "KP1",
        80 => "KP2",
        81 => "KP3",
        82 => "KP0",
        83 => "KPDOT",
        87 => "F11",
        88 => "F12",
        89 => "RO",
        96 => "KPENTER",
        97 => "RIGHTCTRL",
        98 => "KPSLASH",
        99 => "SYSRQ",
        100 => "RIGHTALT",
        101 => "LINEFEED",
        102 => "HOME",
        103 => "UP",
        _ => "UNKNOWN",
    }
}
