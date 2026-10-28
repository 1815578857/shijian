use std::sync::OnceLock;

use tokio::sync::Notify;

static UNLOCK_NOTIFY: OnceLock<Notify> = OnceLock::new();

/// 维护循环等待的解锁信号源；窗口过程无法捕获环境，只能通过这个全局静态投递。
pub fn unlock_notify() -> &'static Notify {
    UNLOCK_NOTIFY.get_or_init(Notify::new)
}

/// 启动一个专用线程监听本会话的“解锁屏幕”事件。
/// 事件只投递信号，是否真正提醒由维护循环按当前设置决定。
pub fn spawn_unlock_watcher() {
    // 先初始化信号静态量，再启动线程；窗口过程只通过这个静态量投递事件。
    unlock_notify();
    if std::thread::Builder::new()
        .name("shijian-unlock-watcher".into())
        .spawn(watcher_thread)
        .is_err()
    {
        eprintln!("无法启动解锁监听线程；桌面提醒将只按固定周期检查");
    }
}

#[cfg(windows)]
fn watcher_thread() {
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::System::RemoteDesktop::{
        WTSRegisterSessionNotification, NOTIFY_FOR_THIS_SESSION,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, RegisterClassW,
        TranslateMessage, HWND_MESSAGE, MSG, WM_WTSSESSION_CHANGE, WNDCLASSW,
        WTS_SESSION_UNLOCK,
    };

    // message-only 窗口：不显示、不可点击，只用来接收系统会话广播。
    unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if msg == WM_WTSSESSION_CHANGE && wparam == WTS_SESSION_UNLOCK as usize {
            if let Some(notify) = UNLOCK_NOTIFY.get() {
                notify.notify_one();
            }
        }
        unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
    }

    let class_name: Vec<u16> = "ShijianUnlockWatcher\0".encode_utf16().collect();
    let class = WNDCLASSW {
        style: 0,
        lpfnWndProc: Some(wndproc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: unsafe { GetModuleHandleW(std::ptr::null()) },
        hIcon: std::ptr::null_mut(),
        hCursor: std::ptr::null_mut(),
        hbrBackground: std::ptr::null_mut(),
        lpszMenuName: std::ptr::null(),
        lpszClassName: class_name.as_ptr(),
    };
    let result = unsafe { RegisterClassW(&class) };
    if result == 0 {
        eprintln!("无法注册解锁监听窗口类；桌面提醒将只按固定周期检查");
        return;
    }
    let hwnd = unsafe {
        CreateWindowExW(
            0,
            class_name.as_ptr(),
            std::ptr::null(),
            0,
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null(),
        )
    };
    if hwnd.is_null() {
        eprintln!("无法创建解锁监听窗口；桌面提醒将只按固定周期检查");
        return;
    }
    // 非交互会话（如服务进程）中注册会失败：只影响解锁触发，固定周期提醒不受影响。
    let registered = unsafe { WTSRegisterSessionNotification(hwnd, NOTIFY_FOR_THIS_SESSION) };
    if registered == 0 {
        eprintln!("无法订阅本会话解锁事件；桌面提醒将只按固定周期检查");
        return;
    }
    let mut message: MSG = unsafe { std::mem::zeroed() };
    while unsafe { GetMessageW(&mut message, hwnd, 0, 0) } > 0 {
        unsafe {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}

#[cfg(not(windows))]
fn watcher_thread(_: &'static Notify) {
    eprintln!("解锁屏幕提醒目前仅支持 Windows");
}
