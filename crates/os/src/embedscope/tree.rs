//! A coherent metadata snapshot from the application's live LVGL instance.
use alloc::{
    string::{String, ToString},
    vec::Vec,
};
use core::{ffi::CStr, ptr::NonNull};
use escope_lvgl::TreeSource;
use escope_protocol::{Status, UiNode};
use rs_lvgl::sys;

// rs_lvgl_sys deliberately omits global variables from its generated bindings.
// These class descriptors are exported by the same native LVGL provider.
unsafe extern "C" {
    static lv_label_class: sys::lv_obj_class_t;
    static lv_button_class: sys::lv_obj_class_t;
    static lv_switch_class: sys::lv_obj_class_t;
    static lv_slider_class: sys::lv_obj_class_t;
    static lv_image_class: sys::lv_obj_class_t;
    static lv_textarea_class: sys::lv_obj_class_t;
    static lv_canvas_class: sys::lv_obj_class_t;
}

const MAX_NODES: usize = 512;
const MAX_TEXT: usize = 160;
#[derive(PartialEq)]
struct Node {
    token: usize,
    parent: u32,
    kind: &'static str,
    name: String,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    flags: u32,
}
pub(super) struct Tree {
    nodes: Vec<Node>,
    revision: u32,
}
impl Tree {
    pub fn new() -> Self {
        Self {
            nodes: Vec::new(),
            revision: 1,
        }
    }
    /// # Safety
    /// Run only on the UI thread, outside native callbacks and mutation.
    /// All roots and descendants remain live throughout this synchronous walk.
    pub unsafe fn refresh(
        &mut self,
        roots: &[NonNull<sys::lv_obj_t>],
        page: &str,
    ) -> Result<(), Status> {
        let mut objects = Vec::new();
        for root in roots {
            objects.push((root.as_ptr(), 0));
        }
        let mut next = Vec::new();
        let mut index = 0;
        while index < objects.len() {
            let (object, parent) = objects[index];
            let mut area = unsafe { core::mem::zeroed::<sys::lv_area_t>() };
            unsafe { sys::lv_obj_get_coords(object, &mut area) };
            let (kind, mut name) = unsafe { metadata(object) };
            if parent == 0 {
                name = page.to_string();
            }
            next.push(Node {
                token: object as usize,
                parent,
                kind,
                name,
                x: area.x1,
                y: area.y1,
                width: (area.x2 as i64 - area.x1 as i64 + 1).max(0) as u32,
                height: (area.y2 as i64 - area.y1 as i64 + 1).max(0) as u32,
                flags: unsafe { sys::lv_obj_get_state(object) },
            });
            let count = unsafe { sys::lv_obj_get_child_count(object) };
            for child_index in 0..count {
                if objects.len() == MAX_NODES {
                    return Err(Status::Capacity);
                }
                let child = unsafe { sys::lv_obj_get_child(object, child_index as i32) };
                if child.is_null() {
                    return Err(Status::Stale);
                }
                objects.push((child, index as u32 + 1));
            }
            index += 1;
        }
        if self.nodes != next {
            self.revision = self.revision.checked_add(1).ok_or(Status::Capacity)?;
            self.nodes = next;
        }
        Ok(())
    }
}
impl TreeSource for Tree {
    fn revision(&self) -> u32 {
        self.revision
    }
    fn read_node(&mut self, cursor: u32) -> Result<UiNode<'_>, Status> {
        let n = self
            .nodes
            .get(cursor as usize)
            .ok_or(Status::InvalidArgument)?;
        Ok(UiNode {
            id: cursor + 1,
            parent: n.parent,
            kind: n.kind,
            name: &n.name,
            x: n.x,
            y: n.y,
            width: n.width,
            height: n.height,
            flags: n.flags,
            revision: self.revision,
            next_cursor: (cursor as usize + 1 < self.nodes.len()).then_some(cursor + 1),
        })
    }
}
unsafe fn metadata(object: *mut sys::lv_obj_t) -> (&'static str, String) {
    let label = unsafe { sys::lv_obj_check_type(object, core::ptr::addr_of!(lv_label_class)) };
    if label {
        let text = unsafe { sys::lv_label_get_text(object) };
        let name = if text.is_null() {
            String::new()
        } else {
            let text = unsafe { CStr::from_ptr(text) }.to_str().unwrap_or("");
            let mut count = text.len().min(MAX_TEXT);
            while !text.is_char_boundary(count) {
                count -= 1;
            }
            text[..count].to_string()
        };
        return ("label", name);
    }
    let classes = [
        (core::ptr::addr_of!(lv_button_class), "button"),
        (core::ptr::addr_of!(lv_switch_class), "switch"),
        (core::ptr::addr_of!(lv_slider_class), "slider"),
        (core::ptr::addr_of!(lv_image_class), "image"),
        (core::ptr::addr_of!(lv_textarea_class), "textarea"),
        (core::ptr::addr_of!(lv_canvas_class), "canvas"),
    ];
    for (class, kind) in classes {
        if unsafe { sys::lv_obj_check_type(object, class) } {
            return (kind, String::new());
        }
    }
    ("lv_obj", String::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_labels_are_owned_and_revisioned_after_deletion() {
        let _guard = crate::test_support::lock();
        rs_lvgl::init();
        let display = rs_lvgl::Display::register::<_, { 32 * 32 }>(32, 32, |_| {}).unwrap();
        unsafe {
            let root = NonNull::new(sys::lv_screen_active()).unwrap();
            let label = sys::lv_label_create(root.as_ptr());
            sys::lv_label_set_text(label, c"设置".as_ptr());
            sys::lv_obj_update_layout(root.as_ptr());
            let mut tree = Tree::new();
            tree.refresh(&[root], "page=Launcher").unwrap();
            let revision = tree.revision();
            assert_eq!(tree.read_node(1).unwrap().name, "设置");
            sys::lv_obj_delete(label);
            // The active traversal reads owned metadata after native deletion.
            assert_eq!(tree.read_node(1).unwrap().name, "设置");
            tree.refresh(&[root], "page=Launcher").unwrap();
            assert!(tree.revision() > revision);
            assert!(tree.read_node(1).is_err());
            drop(tree);
            display.delete();
            rs_lvgl::deinit();
        }
    }
}
