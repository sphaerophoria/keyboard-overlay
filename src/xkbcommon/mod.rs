use std::{
    fs::File,
    io::{BufReader, Error as IoError, Read},
    path::Path,
};

use super::{KeyPress, KeyPressState};

mod bindings;

#[derive(Debug)]
pub enum XkbCreationError {
    ContextCreationFailed,
    OpenMappings(IoError),
    ReadMappings(IoError),
    KeymapCreationFailed,
    StateCreationFailed,
}

macro_rules! xkb_ptr_wrapper {
    ($name:ident, $inner:ty, $drop_fn:expr) => {
        struct $name(*mut $inner);

        impl $name {
            fn new(inner: *mut $inner) -> Option<$name> {
                if inner.is_null() {
                    return None;
                }

                Some($name(inner))
            }

            #[allow(unused)]
            fn as_ptr(&mut self) -> *mut $inner {
                self.0
            }
        }

        impl Drop for $name {
            fn drop(&mut self) {
                unsafe {
                    $drop_fn(self.0);
                }
            }
        }
    };
}

xkb_ptr_wrapper!(Context, bindings::xkb_context, bindings::xkb_context_unref);
xkb_ptr_wrapper!(KeyMap, bindings::xkb_keymap, bindings::xkb_keymap_unref);
xkb_ptr_wrapper!(State, bindings::xkb_state, bindings::xkb_state_unref);

pub struct Xkb {
    state: State,
}

impl Xkb {
    pub fn new(xkb_mapping_path: &Path) -> Result<Xkb, XkbCreationError> {
        unsafe {
            let mut context = create_context()?;
            let mut keymap = create_keymap(&mut context, xkb_mapping_path)?;

            // NOTE: state will hold a reference to a keymap, which wil hold a reference to the
            // context, so we do not need to explicitly hold a reference to the context/keymaps
            // unless we want to use them
            let state = create_state(&mut keymap)?;

            Ok(Xkb { state })
        }
    }

    pub fn push_keycode(&mut self, keycode: u16, press_state: &KeyPressState) -> Option<KeyPress> {
        let xkb_code = evdev_code_to_xkb_code(keycode);

        unsafe {
            update_xkb_state(&mut self.state, xkb_code, press_state);

            let sym = bindings::xkb_state_key_get_one_sym(self.state.as_ptr(), xkb_code);
            keysym_to_keypress(sym)
        }
    }
}

unsafe fn create_context() -> Result<Context, XkbCreationError> {
    Context::new(bindings::xkb_context_new(
        bindings::xkb_context_flags_XKB_CONTEXT_NO_FLAGS,
    ))
    .ok_or(XkbCreationError::ContextCreationFailed)
}

unsafe fn create_keymap(
    context: &mut Context,
    xkb_mapping_path: &Path,
) -> Result<KeyMap, XkbCreationError> {
    let mut f =
        BufReader::new(File::open(xkb_mapping_path).map_err(XkbCreationError::OpenMappings)?);

    let mut mapping_str = Vec::new();
    f.read_to_end(&mut mapping_str)
        .map_err(XkbCreationError::ReadMappings)?;

    KeyMap::new(bindings::xkb_keymap_new_from_buffer(
        context.as_ptr(),
        mapping_str.as_ptr() as *const i8,
        mapping_str.len(),
        bindings::xkb_keymap_format_XKB_KEYMAP_FORMAT_TEXT_V1,
        bindings::xkb_keymap_compile_flags_XKB_KEYMAP_COMPILE_NO_FLAGS,
    ))
    .ok_or(XkbCreationError::KeymapCreationFailed)
}

unsafe fn create_state(keymap: &mut KeyMap) -> Result<State, XkbCreationError> {
    State::new(bindings::xkb_state_new(keymap.as_ptr()))
        .ok_or(XkbCreationError::StateCreationFailed)
}

fn evdev_code_to_xkb_code(code: u16) -> u32 {
    const EVDEV_OFFSET: u32 = 8;
    code as u32 + EVDEV_OFFSET
}

unsafe fn update_xkb_state(state: &mut State, xkb_code: u32, press_state: &KeyPressState) {
    let direction = match press_state {
        KeyPressState::Down => bindings::xkb_key_direction_XKB_KEY_DOWN,
        KeyPressState::Up => bindings::xkb_key_direction_XKB_KEY_UP,
    };

    bindings::xkb_state_update_key(state.as_ptr(), xkb_code, direction);
}

unsafe fn keysym_to_keyname(sym: bindings::xkb_keysym_t) -> Option<String> {
    let mut buf = vec![0; 64];

    let len = bindings::xkb_keysym_get_name(sym, buf.as_mut_ptr() as *mut i8, buf.len());

    if len < 0 {
        return None;
    }

    buf.resize((len + 1) as usize, 0);
    let s = std::ffi::CString::from_vec_with_nul(buf).unwrap();
    Some(s.to_string_lossy().to_string())
}

unsafe fn keysym_to_utf8_name(sym: bindings::xkb_keysym_t) -> Option<String> {
    let mut buf = vec![0; 64];
    let len = bindings::xkb_keysym_to_utf8(sym, buf.as_mut_ptr() as *mut i8, buf.len());
    if len <= 0 {
        return None;
    }
    buf.resize(len as usize, 0);
    let s = std::ffi::CString::from_vec_with_nul(buf).unwrap();
    Some(s.to_string_lossy().to_string())
}

unsafe fn is_unprintable(sym: bindings::xkb_keysym_t) -> bool {
    // Some keys result in unprintable characters but are still valid UTF-8
    matches!(
        sym,
        bindings::XKB_KEY_Escape | bindings::XKB_KEY_Delete | bindings::XKB_KEY_BackSpace
    )
}

unsafe fn keysym_to_string(sym: bindings::xkb_keysym_t) -> Option<String> {
    if is_unprintable(sym) {
        return keysym_to_keyname(sym);
    }

    let utf_name = keysym_to_utf8_name(sym);

    if let Some(name) = utf_name {
        if !name.trim().is_empty() {
            return Some(name);
        }
    }

    keysym_to_keyname(sym)
}

unsafe fn keysym_to_keypress(sym: bindings::xkb_keysym_t) -> Option<KeyPress> {
    let ret = match sym {
        bindings::XKB_KEY_Control_L | bindings::XKB_KEY_Control_R => KeyPress::Ctrl,
        bindings::XKB_KEY_Shift_L | bindings::XKB_KEY_Shift_R => KeyPress::Shift,
        bindings::XKB_KEY_Alt_L | bindings::XKB_KEY_Alt_R => KeyPress::Alt,
        bindings::XKB_KEY_Meta_L | bindings::XKB_KEY_Meta_R => KeyPress::Super,
        _ => keysym_to_string(sym).map(KeyPress::Other)?,
    };

    Some(ret)
}
