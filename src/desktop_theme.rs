//! Shared native dark palette. Uses documented painting, menu and DWM APIs.
use super::*;
use std::sync::{OnceLock, atomic::AtomicUsize};
use windows_sys::Win32::{
    Graphics::Dwm::{DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute},
    UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass},
};

const fn rgb(r: u32, g: u32, b: u32) -> u32 {
    r | (g << 8) | (b << 16)
}
pub const BACKGROUND: u32 = rgb(16, 23, 32);
pub const SURFACE: u32 = rgb(23, 33, 46);
pub const BORDER: u32 = rgb(50, 67, 87);
pub const TEXT: u32 = rgb(232, 239, 248);
pub const MUTED: u32 = rgb(155, 173, 195);
pub const DISABLED: u32 = rgb(98, 115, 137);
pub const ACCENT: u32 = rgb(63, 197, 224);
pub const SUCCESS: u32 = rgb(95, 218, 166);
pub const DANGER: u32 = rgb(249, 135, 148);
const HOVER: u32 = rgb(36, 53, 73);

pub fn background() -> HBRUSH {
    static BRUSH: OnceLock<isize> = OnceLock::new();
    *BRUSH.get_or_init(|| unsafe { CreateSolidBrush(BACKGROUND) as isize }) as HBRUSH
}
fn surface() -> HBRUSH {
    static BRUSH: OnceLock<isize> = OnceLock::new();
    *BRUSH.get_or_init(|| unsafe { CreateSolidBrush(SURFACE) as isize }) as HBRUSH
}
fn fill(dc: HDC, rect: &RECT, color: u32) {
    unsafe {
        let brush = CreateSolidBrush(color);
        FillRect(dc, rect, brush);
        DeleteObject(brush);
    }
}
fn text(dc: HDC, value: &str, mut rect: RECT, color: u32, flags: u32) {
    unsafe {
        SetBkMode(dc, TRANSPARENT as i32);
        SetTextColor(dc, color);
        DrawTextW(dc, wide(value).as_ptr(), -1, &mut rect, flags);
    }
}
pub fn window(window: HWND) {
    unsafe {
        let enabled = 1i32;
        DwmSetWindowAttribute(
            window,
            DWMWA_USE_IMMERSIVE_DARK_MODE as u32,
            (&enabled as *const i32).cast(),
            size_of::<i32>() as u32,
        );
        menu(GetMenu(window), true);
    }
}
pub fn redraw(window: HWND, immediate: bool) {
    unsafe {
        RedrawWindow(
            window,
            null(),
            null_mut(),
            RDW_INVALIDATE
                | RDW_ERASE
                | RDW_FRAME
                | RDW_ALLCHILDREN
                | if immediate { RDW_UPDATENOW } else { 0 },
        );
    }
}
/// WM_SETREDRAW changes WS_VISIBLE; preserve a hidden tab's controls as well.
pub struct RedrawGuard {
    window: HWND,
    visible: bool,
}
impl RedrawGuard {
    pub fn new(window: HWND) -> Self {
        let visible = unsafe { GetWindowLongPtrW(window, GWL_STYLE) as u32 & WS_VISIBLE != 0 };
        unsafe {
            SendMessageW(window, WM_SETREDRAW, 0, 0);
        }
        Self { window, visible }
    }
}
impl Drop for RedrawGuard {
    fn drop(&mut self) {
        unsafe {
            SendMessageW(self.window, WM_SETREDRAW, 1, 0);
            if !self.visible {
                ShowWindow(self.window, SW_HIDE);
            }
        }
        redraw(self.window, false);
    }
}
pub fn control(window: HWND, class: &str) {
    unsafe {
        match class {
            "BUTTON"
                if GetWindowLongPtrW(window, GWL_STYLE) as u32 & 0xf == BS_AUTOCHECKBOX as u32 =>
            {
                SetWindowSubclass(window, Some(paint_control), 1, 5);
            }
            "SysListView32" => {
                SetWindowTheme(window, wide("").as_ptr(), wide("").as_ptr());
                SendMessageW(window, LVM_SETBKCOLOR, 0, SURFACE as isize);
                SendMessageW(window, LVM_SETTEXTBKCOLOR, 0, SURFACE as isize);
                SendMessageW(window, LVM_SETTEXTCOLOR, 0, TEXT as isize);
                let header = SendMessageW(window, LVM_GETHEADER, 0, 0) as HWND;
                SetWindowSubclass(header, Some(paint_control), 1, 2);
            }
            "SysTreeView32" => {
                SetWindowTheme(window, wide("").as_ptr(), wide("").as_ptr());
                SendMessageW(window, TVM_SETBKCOLOR, 0, SURFACE as isize);
                SendMessageW(window, TVM_SETTEXTCOLOR, 0, TEXT as isize);
                SendMessageW(window, TVM_SETLINECOLOR, 0, BORDER as isize);
            }
            "SysTabControl32" => {
                SetWindowSubclass(window, Some(paint_control), 1, 1);
            }
            "COMBOBOX" => {
                SetWindowSubclass(window, Some(paint_control), 1, 3);
            }
            "EDIT" if GetWindowLongPtrW(window, GWL_STYLE) as u32 & ES_MULTILINE as u32 != 0 => {
                SetWindowSubclass(window, Some(paint_control), 1, 4);
            }
            _ => {}
        }
    }
}
fn combo_text(window: HWND, index: usize) -> String {
    unsafe {
        let length = SendMessageW(window, CB_GETLBTEXTLEN, index, 0);
        if !(0..4096).contains(&length) {
            return String::new();
        }
        let mut data = vec![0; length as usize + 1];
        SendMessageW(window, CB_GETLBTEXT, index, data.as_mut_ptr() as isize);
        String::from_utf16_lossy(&data[..length as usize])
    }
}
unsafe extern "system" fn paint_control(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    id: usize,
    kind: usize,
) -> LRESULT {
    if message == WM_NCDESTROY {
        unsafe {
            RemoveWindowSubclass(window, Some(paint_control), id);
        }
    }
    if kind != 4 && matches!(message, WM_PAINT | WM_PRINTCLIENT | WM_ERASEBKGND) {
        unsafe {
            if message == WM_ERASEBKGND {
                return 1;
            }
            let mut paint: PAINTSTRUCT = std::mem::zeroed();
            let dc = if message == WM_PAINT {
                BeginPaint(window, &mut paint)
            } else {
                wparam as HDC
            };
            let saved = SaveDC(dc);
            SelectObject(dc, SendMessageW(window, WM_GETFONT, 0, 0) as HGDIOBJ);
            let mut rect: RECT = std::mem::zeroed();
            GetClientRect(window, &mut rect);
            fill(dc, &rect, if kind == 1 { BACKGROUND } else { SURFACE });
            if kind == 5 {
                fill(dc, &rect, BACKGROUND);
                let enabled =
                    windows_sys::Win32::UI::Input::KeyboardAndMouse::IsWindowEnabled(window) != 0;
                let checked = SendMessageW(window, BM_GETCHECK, 0, 0) == BST_CHECKED as isize;
                let size = scale(window, 16);
                let box_rect = RECT {
                    left: scale(window, 2),
                    top: (rect.bottom - size) / 2,
                    right: scale(window, 2) + size,
                    bottom: (rect.bottom + size) / 2,
                };
                fill(
                    dc,
                    &box_rect,
                    if !enabled {
                        DISABLED
                    } else if checked {
                        ACCENT
                    } else {
                        BORDER
                    },
                );
                if checked {
                    text(
                        dc,
                        "✓",
                        box_rect,
                        BACKGROUND,
                        DT_SINGLELINE | DT_VCENTER | DT_CENTER,
                    );
                } else {
                    fill(
                        dc,
                        &RECT {
                            left: box_rect.left + 1,
                            top: box_rect.top + 1,
                            right: box_rect.right - 1,
                            bottom: box_rect.bottom - 1,
                        },
                        SURFACE,
                    );
                }
                text(
                    dc,
                    &window_text(window),
                    RECT {
                        left: box_rect.right + scale(window, 8),
                        ..rect
                    },
                    if enabled { TEXT } else { DISABLED },
                    DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX,
                );
                if windows_sys::Win32::UI::Input::KeyboardAndMouse::GetFocus() == window {
                    DrawFocusRect(dc, &rect);
                }
            } else if kind == 3 {
                let selected = SendMessageW(window, CB_GETCURSEL, 0, 0);
                let enabled =
                    windows_sys::Win32::UI::Input::KeyboardAndMouse::IsWindowEnabled(window) != 0;
                let mut label = rect;
                label.left += scale(window, 10);
                label.right -= scale(window, 30);
                text(
                    dc,
                    &combo_text(window, selected as usize),
                    label,
                    if enabled { TEXT } else { DISABLED },
                    DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
                );
                let arrow = RECT {
                    left: rect.right - scale(window, 26),
                    ..rect
                };
                text(
                    dc,
                    "⌄",
                    arrow,
                    if enabled { MUTED } else { DISABLED },
                    DT_SINGLELINE | DT_VCENTER | DT_CENTER,
                );
                let pen = CreatePen(PS_SOLID, 1, BORDER);
                let old = SelectObject(dc, pen);
                let brush = SelectObject(dc, GetStockObject(NULL_BRUSH));
                Rectangle(dc, rect.left, rect.top, rect.right, rect.bottom);
                SelectObject(dc, brush);
                SelectObject(dc, old);
                DeleteObject(pen);
            } else {
                let count = SendMessageW(
                    window,
                    if kind == 1 {
                        TCM_GETITEMCOUNT
                    } else {
                        HDM_GETITEMCOUNT
                    },
                    0,
                    0,
                );
                let selected = SendMessageW(window, TCM_GETCURSEL, 0, 0);
                for index in 0..count.max(0) as usize {
                    let mut bounds: RECT = std::mem::zeroed();
                    SendMessageW(
                        window,
                        if kind == 1 {
                            TCM_GETITEMRECT
                        } else {
                            HDM_GETITEMRECT
                        },
                        index,
                        (&mut bounds as *mut RECT) as isize,
                    );
                    let mut value = [0u16; 256];
                    if kind == 1 {
                        let mut item = TCITEMW {
                            mask: TCIF_TEXT,
                            pszText: value.as_mut_ptr(),
                            cchTextMax: 256,
                            ..std::mem::zeroed()
                        };
                        SendMessageW(
                            window,
                            TCM_GETITEMW,
                            index,
                            (&mut item as *mut TCITEMW) as isize,
                        );
                        if selected == index as isize {
                            fill(dc, &bounds, SURFACE);
                            let line = RECT {
                                top: bounds.bottom - scale(window, 3),
                                ..bounds
                            };
                            fill(dc, &line, ACCENT);
                        }
                    } else {
                        let mut item = HDITEMW {
                            mask: HDI_TEXT,
                            pszText: value.as_mut_ptr(),
                            cchTextMax: 256,
                            ..std::mem::zeroed()
                        };
                        SendMessageW(
                            window,
                            HDM_GETITEMW,
                            index,
                            (&mut item as *mut HDITEMW) as isize,
                        );
                    }
                    bounds.left += scale(window, 12);
                    bounds.right -= scale(window, 10);
                    let end = value.iter().position(|v| *v == 0).unwrap_or(value.len());
                    text(
                        dc,
                        &String::from_utf16_lossy(&value[..end]),
                        bounds,
                        if kind == 1 && selected == index as isize {
                            TEXT
                        } else {
                            MUTED
                        },
                        DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
                    );
                }
            }
            RestoreDC(dc, saved);
            if message == WM_PAINT {
                EndPaint(window, &paint);
            }
            return 0;
        }
    }
    let result = unsafe { DefSubclassProc(window, message, wparam, lparam) };
    if kind == 4 && matches!(message, WM_SIZE | WM_SETTEXT | WM_SETFONT) {
        unsafe {
            let mut rect: RECT = std::mem::zeroed();
            GetClientRect(window, &mut rect);
            let dc = GetDC(window);
            let old = SelectObject(dc, SendMessageW(window, WM_GETFONT, 0, 0) as HGDIOBJ);
            let mut metrics = std::mem::zeroed();
            GetTextMetricsW(dc, &mut metrics);
            SelectObject(dc, old);
            ReleaseDC(window, dc);
            let lines = SendMessageW(window, EM_GETLINECOUNT, 0, 0);
            ShowScrollBar(
                window,
                SB_VERT,
                i32::from(
                    lines * metrics.tmHeight.max(1) as isize
                        > rect.bottom.saturating_sub(4) as isize,
                ),
            );
        }
    }
    if matches!(message, WM_ENABLE | WM_SETFOCUS | WM_KILLFOCUS)
        || (kind == 3 && message == CB_SETCURSEL)
        || (kind == 5 && matches!(message, BM_SETCHECK | WM_LBUTTONUP | WM_KEYUP))
    {
        unsafe {
            InvalidateRect(window, null(), 0);
        }
    }
    result
}

pub fn notification(lparam: LPARAM) -> Option<LRESULT> {
    unsafe {
        let header = &*(lparam as *const NMHDR);
        if header.code != NM_CUSTOMDRAW {
            return None;
        }
        let mut class = [0u16; 32];
        let length = GetClassNameW(header.hwndFrom, class.as_mut_ptr(), 32);
        let class = String::from_utf16_lossy(&class[..length.max(0) as usize]);
        if class != "SysTreeView32" && class != "SysListView32" {
            return None;
        }
        let draw = &mut *(lparam as *mut NMCUSTOMDRAW);
        match draw.dwDrawStage {
            CDDS_PREPAINT => Some(CDRF_NOTIFYITEMDRAW as isize),
            CDDS_ITEMPREPAINT => {
                if class == "SysListView32" {
                    let saved = SaveDC(draw.hdc);
                    SelectObject(
                        draw.hdc,
                        SendMessageW(header.hwndFrom, WM_GETFONT, 0, 0) as HGDIOBJ,
                    );
                    let selected = SendMessageW(
                        header.hwndFrom,
                        LVM_GETITEMSTATE,
                        draw.dwItemSpec,
                        LVIS_SELECTED as isize,
                    ) & LVIS_SELECTED as isize
                        != 0;
                    let mut row = draw.rc;
                    let mut client: RECT = std::mem::zeroed();
                    GetClientRect(header.hwndFrom, &mut client);
                    row.left = 0;
                    row.right = client.right;
                    fill(draw.hdc, &row, if selected { HOVER } else { SURFACE });
                    if selected {
                        fill(
                            draw.hdc,
                            &RECT {
                                right: scale(header.hwndFrom, 3),
                                ..row
                            },
                            ACCENT,
                        );
                    }
                    let columns = SendMessageW(
                        SendMessageW(header.hwndFrom, LVM_GETHEADER, 0, 0) as HWND,
                        HDM_GETITEMCOUNT,
                        0,
                        0,
                    );
                    let mut x = -GetScrollPos(header.hwndFrom, SB_HORZ);
                    for column in 0..columns.max(0) as usize {
                        let width =
                            SendMessageW(header.hwndFrom, LVM_GETCOLUMNWIDTH, column, 0) as i32;
                        let mut value = [0u16; 512];
                        let mut item = LVITEMW {
                            iSubItem: column as i32,
                            pszText: value.as_mut_ptr(),
                            cchTextMax: 512,
                            ..std::mem::zeroed()
                        };
                        let length = SendMessageW(
                            header.hwndFrom,
                            LVM_GETITEMTEXTW,
                            draw.dwItemSpec,
                            (&mut item as *mut LVITEMW) as isize,
                        )
                        .max(0) as usize;
                        let checkbox = column == 0
                            && SendMessageW(header.hwndFrom, LVM_GETEXTENDEDLISTVIEWSTYLE, 0, 0)
                                & LVS_EX_CHECKBOXES as isize
                                != 0;
                        let mut icon_width = 0;
                        let images = SendMessageW(
                            header.hwndFrom,
                            LVM_GETIMAGELIST,
                            LVSIL_SMALL as usize,
                            0,
                        ) as HIMAGELIST;
                        if column == 1 && images != 0 {
                            let mut image = LVITEMW {
                                mask: LVIF_IMAGE,
                                iItem: draw.dwItemSpec as i32,
                                iSubItem: 1,
                                ..std::mem::zeroed()
                            };
                            if SendMessageW(
                                header.hwndFrom,
                                LVM_GETITEMW,
                                0,
                                (&mut image as *mut LVITEMW) as isize,
                            ) != 0
                                && image.iImage >= 0
                            {
                                let mut iw = 0;
                                let mut ih = 0;
                                ImageList_GetIconSize(images, &mut iw, &mut ih);
                                ImageList_Draw(
                                    images,
                                    image.iImage,
                                    draw.hdc,
                                    x + scale(header.hwndFrom, 8),
                                    (row.top + row.bottom - ih) / 2,
                                    ILD_TRANSPARENT,
                                );
                                icon_width = iw + scale(header.hwndFrom, 6);
                            }
                        }
                        if checkbox {
                            let size = scale(header.hwndFrom, 16);
                            let rect = RECT {
                                left: x + scale(header.hwndFrom, 2),
                                right: x + scale(header.hwndFrom, 2) + size,
                                top: (row.top + row.bottom - size) / 2,
                                bottom: (row.top + row.bottom - size) / 2 + size,
                            };
                            let checked = SendMessageW(
                                header.hwndFrom,
                                LVM_GETITEMSTATE,
                                draw.dwItemSpec,
                                LVIS_STATEIMAGEMASK as isize,
                            ) & LVIS_STATEIMAGEMASK as isize
                                == 2 << 12;
                            fill(draw.hdc, &rect, if checked { ACCENT } else { BORDER });
                            if checked {
                                text(
                                    draw.hdc,
                                    "✓",
                                    rect,
                                    BACKGROUND,
                                    DT_SINGLELINE | DT_CENTER | DT_VCENTER | DT_NOPREFIX,
                                );
                            } else {
                                fill(
                                    draw.hdc,
                                    &RECT {
                                        left: rect.left + 1,
                                        right: rect.right - 1,
                                        top: rect.top + 1,
                                        bottom: rect.bottom - 1,
                                    },
                                    SURFACE,
                                );
                            }
                        }
                        text(
                            draw.hdc,
                            &String::from_utf16_lossy(&value[..length]),
                            RECT {
                                left: x
                                    + scale(header.hwndFrom, if checkbox { 26 } else { 12 })
                                    + icon_width,
                                right: x + width - scale(header.hwndFrom, 10),
                                ..row
                            },
                            TEXT,
                            DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX,
                        );
                        x += width;
                    }
                    RestoreDC(draw.hdc, saved);
                    return Some(CDRF_SKIPDEFAULT as isize);
                }
                let selected = if class == "SysListView32" {
                    SendMessageW(
                        header.hwndFrom,
                        LVM_GETITEMSTATE,
                        draw.dwItemSpec,
                        LVIS_SELECTED as isize,
                    ) & LVIS_SELECTED as isize
                        != 0
                } else {
                    draw.uItemState & CDIS_SELECTED != 0
                };
                draw.uItemState &= !CDIS_SELECTED;
                if class == "SysTreeView32" {
                    let draw = &mut *(lparam as *mut NMTVCUSTOMDRAW);
                    draw.clrText = TEXT;
                    draw.clrTextBk = if selected { HOVER } else { SURFACE };
                } else {
                    let draw = &mut *(lparam as *mut NMLVCUSTOMDRAW);
                    draw.clrText = TEXT;
                    draw.clrTextBk = if selected { HOVER } else { SURFACE };
                }
                Some(CDRF_NEWFONT as isize)
            }
            _ => None,
        }
    }
}

#[derive(Clone)]
struct MenuLabel {
    text: String,
    bar: bool,
    submenu: bool,
}
static LABELS: OnceLock<Mutex<HashMap<usize, MenuLabel>>> = OnceLock::new();
static NEXT_LABEL: AtomicUsize = AtomicUsize::new(1);
fn labels() -> &'static Mutex<HashMap<usize, MenuLabel>> {
    LABELS.get_or_init(Mutex::default)
}
pub fn menu(handle: HMENU, bar: bool) {
    if handle.is_null() {
        return;
    }
    unsafe {
        let info = MENUINFO {
            cbSize: size_of::<MENUINFO>() as u32,
            fMask: MIM_BACKGROUND,
            hbrBack: background(),
            ..std::mem::zeroed()
        };
        SetMenuInfo(handle, &info);
        for index in 0..GetMenuItemCount(handle).max(0) as u32 {
            let mut value = [0u16; 512];
            let mut item = MENUITEMINFOW {
                cbSize: size_of::<MENUITEMINFOW>() as u32,
                fMask: MIIM_FTYPE | MIIM_DATA | MIIM_STRING | MIIM_SUBMENU,
                dwTypeData: value.as_mut_ptr(),
                cch: value.len() as u32,
                ..std::mem::zeroed()
            };
            GetMenuItemInfoW(handle, index, 1, &mut item);
            if item.fType & MFT_SEPARATOR != 0 {
                continue;
            }
            if item.fType & MFT_OWNERDRAW == 0 {
                let key = NEXT_LABEL.fetch_add(1, Ordering::Relaxed);
                labels().lock().unwrap().insert(
                    key,
                    MenuLabel {
                        text: String::from_utf16_lossy(&value[..item.cch as usize]),
                        bar,
                        submenu: !item.hSubMenu.is_null(),
                    },
                );
                item.fMask = MIIM_FTYPE | MIIM_DATA;
                item.fType |= MFT_OWNERDRAW;
                item.dwItemData = key;
                if SetMenuItemInfoW(handle, index, 1, &item) == 0 {
                    labels().lock().unwrap().remove(&key);
                }
            }
            menu(item.hSubMenu, false);
        }
    }
}
fn forget_menu(handle: HMENU) {
    if handle.is_null() {
        return;
    }
    unsafe {
        for index in 0..GetMenuItemCount(handle).max(0) as u32 {
            let mut item = MENUITEMINFOW {
                cbSize: size_of::<MENUITEMINFOW>() as u32,
                fMask: MIIM_DATA | MIIM_SUBMENU,
                ..std::mem::zeroed()
            };
            GetMenuItemInfoW(handle, index, 1, &mut item);
            labels().lock().unwrap().remove(&item.dwItemData);
            forget_menu(item.hSubMenu);
        }
    }
}
pub fn destroy_menu(handle: HMENU) {
    forget_menu(handle);
    unsafe {
        DestroyMenu(handle);
    }
}
pub fn destroy_window(window: HWND) {
    unsafe {
        let menu = GetMenu(window);
        SetMenu(window, null_mut());
        destroy_menu(menu);
        DestroyWindow(window);
    }
}
#[cfg(test)]
pub(crate) fn menu_label_count() -> usize {
    labels().lock().unwrap().len()
}

pub fn message(window: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
    unsafe {
        match message {
            WM_ERASEBKGND => {
                let mut rect: RECT = std::mem::zeroed();
                GetClientRect(window, &mut rect);
                FillRect(wparam as HDC, &rect, background());
                Some(1)
            }
            WM_CTLCOLORSTATIC | WM_CTLCOLOREDIT | WM_CTLCOLORLISTBOX => {
                let dc = wparam as HDC;
                let enabled = windows_sys::Win32::UI::Input::KeyboardAndMouse::IsWindowEnabled(
                    lparam as HWND,
                ) != 0;
                SetTextColor(dc, if enabled { TEXT } else { DISABLED });
                let field = message != WM_CTLCOLORSTATIC || {
                    let mut class = [0u16; 16];
                    GetClassNameW(lparam as HWND, class.as_mut_ptr(), 16);
                    String::from_utf16_lossy(&class).starts_with("EDIT")
                };
                SetBkColor(dc, if field { SURFACE } else { BACKGROUND });
                Some(if field { surface() } else { background() } as isize)
            }
            WM_MEASUREITEM => {
                let item = &mut *(lparam as *mut MEASUREITEMSTRUCT);
                if item.CtlType == ODT_MENU {
                    let label = labels().lock().unwrap().get(&item.itemData).cloned()?;
                    let dc = GetDC(window);
                    let font = font(window);
                    let old = SelectObject(dc, font);
                    let value = wide(&label.text);
                    let mut size = std::mem::zeroed();
                    GetTextExtentPoint32W(dc, value.as_ptr(), value.len() as i32 - 1, &mut size);
                    SelectObject(dc, old);
                    DeleteObject(font);
                    ReleaseDC(window, dc);
                    item.itemWidth = (size.cx + scale(window, if label.bar { 20 } else { 48 }))
                        .min(scale(window, 520)) as u32;
                    item.itemHeight = scale(window, if label.bar { 24 } else { 32 }) as u32;
                    Some(1)
                } else if item.CtlType == ODT_COMBOBOX {
                    item.itemHeight = scale(window, 28) as u32;
                    Some(1)
                } else {
                    None
                }
            }
            WM_DRAWITEM => {
                let item = &*(lparam as *const DRAWITEMSTRUCT);
                if !matches!(
                    item.CtlType,
                    ODT_MENU | ODT_BUTTON | ODT_COMBOBOX | ODT_STATIC
                ) {
                    return None;
                }
                let dc = item.hDC;
                let saved = SaveDC(dc);
                let disabled = item.itemState & (ODS_DISABLED | ODS_GRAYED) != 0;
                let selected = item.itemState & (ODS_SELECTED | ODS_HOTLIGHT) != 0;
                let mut rect = item.rcItem;
                if item.CtlType == ODT_STATIC {
                    fill(dc, &rect, BACKGROUND);
                    SelectObject(dc, SendMessageW(item.hwndItem, WM_GETFONT, 0, 0) as HGDIOBJ);
                    let enabled = windows_sys::Win32::UI::Input::KeyboardAndMouse::IsWindowEnabled(
                        item.hwndItem,
                    ) != 0;
                    text(
                        dc,
                        &window_text(item.hwndItem),
                        rect,
                        if enabled { TEXT } else { DISABLED },
                        DT_WORDBREAK | DT_NOPREFIX,
                    );
                    RestoreDC(dc, saved);
                    return Some(1);
                }
                let (value, flags) = if item.CtlType == ODT_MENU {
                    let label = labels().lock().unwrap().get(&item.itemData).cloned();
                    let label = match label {
                        Some(l) => l,
                        None => {
                            RestoreDC(dc, saved);
                            return None;
                        }
                    };
                    fill(
                        dc,
                        &rect,
                        if selected && !disabled {
                            HOVER
                        } else {
                            BACKGROUND
                        },
                    );
                    let font = font(window);
                    SelectObject(dc, font);
                    rect.left += scale(window, if label.bar { 10 } else { 20 });
                    rect.right -= scale(window, 12);
                    text(
                        dc,
                        &label.text,
                        rect,
                        if disabled { DISABLED } else { TEXT },
                        DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
                    );
                    if label.submenu && !label.bar {
                        text(dc, "›", rect, MUTED, DT_SINGLELINE | DT_VCENTER | DT_RIGHT);
                    }
                    RestoreDC(dc, saved);
                    DeleteObject(font);
                    return Some(1);
                } else if item.CtlType == ODT_BUTTON {
                    let accent = matches!(item.CtlID, 1 | 101 | 301 | 303 | 904);
                    let color = if disabled {
                        SURFACE
                    } else if selected {
                        HOVER
                    } else if accent {
                        ACCENT
                    } else {
                        SURFACE
                    };
                    fill(dc, &rect, BACKGROUND);
                    let brush = CreateSolidBrush(color);
                    let pen = CreatePen(
                        PS_SOLID,
                        1,
                        if accent && !disabled { ACCENT } else { BORDER },
                    );
                    let old_b = SelectObject(dc, brush);
                    let old_p = SelectObject(dc, pen);
                    RoundRect(
                        dc,
                        rect.left,
                        rect.top,
                        rect.right,
                        rect.bottom,
                        scale(window, 10),
                        scale(window, 10),
                    );
                    SelectObject(dc, old_b);
                    SelectObject(dc, old_p);
                    DeleteObject(brush);
                    DeleteObject(pen);
                    SelectObject(dc, SendMessageW(item.hwndItem, WM_GETFONT, 0, 0) as HGDIOBJ);
                    rect.left += scale(window, 8);
                    rect.right -= scale(window, 8);
                    text(
                        dc,
                        &window_text(item.hwndItem),
                        rect,
                        if disabled {
                            DISABLED
                        } else if accent && !selected {
                            BACKGROUND
                        } else {
                            TEXT
                        },
                        DT_SINGLELINE | DT_VCENTER | DT_CENTER | DT_END_ELLIPSIS,
                    );
                    if item.itemState & ODS_FOCUS != 0 {
                        InflateRect(&mut rect, -scale(window, 3), -scale(window, 5));
                        DrawFocusRect(dc, &rect);
                    }
                    RestoreDC(dc, saved);
                    return Some(1);
                } else {
                    fill(dc, &rect, if selected { HOVER } else { SURFACE });
                    SelectObject(dc, SendMessageW(item.hwndItem, WM_GETFONT, 0, 0) as HGDIOBJ);
                    (
                        combo_text(item.hwndItem, item.itemID as usize),
                        DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
                    )
                };
                rect.left += scale(window, 10);
                rect.right -= scale(window, 8);
                text(
                    dc,
                    &value,
                    rect,
                    if disabled { DISABLED } else { TEXT },
                    flags,
                );
                RestoreDC(dc, saved);
                Some(1)
            }
            WM_DESTROY => {
                forget_menu(GetMenu(window));
                None
            }
            WM_MENUCHAR => {
                let handle = lparam as HMENU;
                let key = char::from_u32((wparam & 0xffff) as u32)?
                    .to_lowercase()
                    .next()?;
                for index in 0..GetMenuItemCount(handle).max(0) as u32 {
                    let mut item = MENUITEMINFOW {
                        cbSize: size_of::<MENUITEMINFOW>() as u32,
                        fMask: MIIM_DATA | MIIM_STATE,
                        ..std::mem::zeroed()
                    };
                    GetMenuItemInfoW(handle, index, 1, &mut item);
                    if item.fState & (MFS_DISABLED | MFS_GRAYED) != 0 {
                        continue;
                    }
                    if labels()
                        .lock()
                        .unwrap()
                        .get(&item.dwItemData)
                        .is_some_and(|label| {
                            label
                                .text
                                .trim_start_matches('&')
                                .to_lowercase()
                                .starts_with(key)
                        })
                    {
                        return Some(index as isize | ((MNC_EXECUTE as isize) << 16));
                    }
                }
                None
            }
            _ => None,
        }
    }
}

#[derive(Default)]
pub struct Fonts {
    pub regular: HFONT,
    pub heading: HFONT,
}
impl Fonts {
    pub fn new(window: HWND) -> Self {
        let regular = font(window);
        let heading = unsafe {
            CreateFontW(
                -scale(window, 24),
                0,
                0,
                0,
                FW_SEMIBOLD as i32,
                0,
                0,
                0,
                DEFAULT_CHARSET as u32,
                0,
                0,
                CLEARTYPE_QUALITY as u32,
                0,
                wide("Segoe UI").as_ptr(),
            )
        };
        Self { regular, heading }
    }
    pub fn update(&mut self, window: HWND) {
        let next = Self::new(window);
        unsafe extern "system" fn apply(child: HWND, parameter: LPARAM) -> i32 {
            let fonts = unsafe { &*(parameter as *const Fonts) };
            unsafe {
                apply_font(
                    child,
                    if GetDlgCtrlID(child) == 310 {
                        fonts.heading
                    } else {
                        fonts.regular
                    },
                );
            }
            1
        }
        unsafe {
            EnumChildWindows(window, Some(apply), (&next as *const Self) as isize);
        }
        *self = next;
    }
}
impl Drop for Fonts {
    fn drop(&mut self) {
        unsafe {
            DeleteObject(self.regular);
            DeleteObject(self.heading);
        }
    }
}
pub fn dpi_rect(window: HWND, lparam: LPARAM) {
    unsafe {
        let rect = &*(lparam as *const RECT);
        SetWindowPos(
            window,
            null_mut(),
            rect.left,
            rect.top,
            rect.right - rect.left,
            rect.bottom - rect.top,
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_table_scrolls_and_refresh_preserves_hidden_tab_visibility() {
        unsafe {
            InitCommonControlsEx(&INITCOMMONCONTROLSEX {
                dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32,
                dwICC: ICC_LISTVIEW_CLASSES,
            });
            let parent = CreateWindowExW(
                0,
                wide("STATIC").as_ptr(),
                wide("").as_ptr(),
                WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
                -10000,
                -10000,
                480,
                260,
                null_mut(),
                null_mut(),
                GetModuleHandleW(null()),
                null(),
            );
            assert!(!parent.is_null());
            struct Window(HWND);
            impl Drop for Window {
                fn drop(&mut self) {
                    unsafe {
                        DestroyWindow(self.0);
                    }
                }
            }
            let _cleanup = Window(parent);
            let list = super::super::control(
                parent,
                "SysListView32",
                "",
                WS_VSCROLL | WS_HSCROLL | WS_BORDER | LVS_REPORT,
                313,
            );
            MoveWindow(list, 0, 0, 400, 140, 0);
            for index in 0..5 {
                let mut label = wide("Column");
                let column = LVCOLUMNW {
                    mask: LVCF_TEXT | LVCF_WIDTH,
                    pszText: label.as_mut_ptr(),
                    cx: 110,
                    ..std::mem::zeroed()
                };
                SendMessageW(
                    list,
                    LVM_INSERTCOLUMNW,
                    index,
                    (&column as *const LVCOLUMNW) as isize,
                );
            }
            {
                let _redraw = RedrawGuard::new(list);
                for index in 0..80 {
                    let mut value = wide(&format!("USB device {index}"));
                    let item = LVITEMW {
                        mask: LVIF_TEXT,
                        iItem: index,
                        pszText: value.as_mut_ptr(),
                        ..std::mem::zeroed()
                    };
                    SendMessageW(list, LVM_INSERTITEMW, 0, (&item as *const LVITEMW) as isize);
                }
            }
            let mut scroll = SCROLLINFO {
                cbSize: size_of::<SCROLLINFO>() as u32,
                fMask: SIF_ALL,
                ..std::mem::zeroed()
            };
            assert_ne!(GetScrollInfo(list, SB_VERT, &mut scroll), 0);
            assert!(scroll.nMax >= 79 && scroll.nPage > 0 && scroll.nPage < 80);
            assert_ne!(SendMessageW(list, LVM_ENSUREVISIBLE, 79, 0), 0);
            assert!(SendMessageW(list, LVM_GETTOPINDEX, 0, 0) > 0);
            ShowWindow(list, SW_HIDE);
            {
                let _redraw = RedrawGuard::new(list);
                assert_eq!(SendMessageW(list, LVM_GETITEMCOUNT, 0, 0), 80);
            }
            assert_eq!(GetWindowLongPtrW(list, GWL_STYLE) as u32 & WS_VISIBLE, 0);
            ShowWindow(list, SW_SHOW);
            {
                let _redraw = RedrawGuard::new(list);
            }
            assert_ne!(GetWindowLongPtrW(list, GWL_STYLE) as u32 & WS_VISIBLE, 0);
        }
    }
}
