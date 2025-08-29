use image::{ImageBuffer, Rgba};
use minifb::{Key, MouseButton, MouseMode, Scale, ScaleMode, Window, WindowOptions};
use screenshots::Screen;
use std::mem::zeroed;
use std::ptr::null_mut;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;
use std::{
    collections::VecDeque,
    time::{SystemTime, UNIX_EPOCH},
};
use winapi::um::winuser::{
    DispatchMessageW, GetMessageW, MOD_ALT, MOD_CONTROL, MSG, PostQuitMessage, RegisterHotKey,
    TranslateMessage, UnregisterHotKey, WM_HOTKEY, WM_QUIT,
};

// 自定义事件枚举
#[derive(Debug, Clone)]
enum AppEvent {
    KeyPressed(Key),
    KeyReleased(Key),
    MousePressed(MouseButton, f32, f32),
    MouseReleased(MouseButton, f32, f32),
    MouseMoved(f32, f32),
    WindowResized(usize, usize),
    GlobalHotkeyPressed, // 新增：全局热键事件
    Quit,                // 退出事件
}

// 程序状态
enum State {
    Idle,
    FullscreenCapture(ImageBuffer<Rgba<u8>, Vec<u8>>),
    SelectingRegion(ImageBuffer<Rgba<u8>, Vec<u8>>, (i32, i32), (i32, i32)),
    RegionSelected(ImageBuffer<Rgba<u8>, Vec<u8>>, (i32, i32, i32, i32)),
}

// 全局热键ID
const HOTKEY_ID: i32 = 1;
const SAVE_HOTKEY_ID: i32 = 2; // 新增保存热键ID

fn main() {
    // 创建通道用于线程间通信
    let (tx, rx): (Sender<AppEvent>, Receiver<AppEvent>) = channel();

    // 启动消息处理线程
    let tx_clone = tx.clone();
    thread::spawn(move || {
        // 注册全局热键: Ctrl+Alt+D 用于截图
        unsafe {
            RegisterHotKey(
                null_mut(),
                HOTKEY_ID,
                MOD_CONTROL as u32 | MOD_ALT as u32,
                'D' as u32,
            );
            // 注册全局热键: Ctrl+S 用于保存
            RegisterHotKey(
                null_mut(),
                SAVE_HOTKEY_ID,
                MOD_CONTROL as u32,
                'S' as u32,
            );
        }

        // Windows 消息循环
        let mut msg: MSG = unsafe { zeroed() };
        loop {
            let result = unsafe { GetMessageW(&mut msg, null_mut(), 0, 0) };
            if result <= 0 {
                break;
            }

            unsafe {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }

            match msg.message {
                WM_HOTKEY => {
                    match msg.wParam as i32 {
                        HOTKEY_ID => {
                            tx_clone.send(AppEvent::GlobalHotkeyPressed).unwrap();
                        }
                        SAVE_HOTKEY_ID => {
                            // 发送保存事件
                            tx_clone.send(AppEvent::KeyPressed(Key::S)).unwrap();
                        }
                        _ => {}
                    }
                }
                WM_QUIT => {
                    tx_clone.send(AppEvent::Quit).unwrap();
                    break;
                }
                _ => {}
            }
        }

        // 取消注册热键
        unsafe {
            UnregisterHotKey(null_mut(), HOTKEY_ID);
            UnregisterHotKey(null_mut(), SAVE_HOTKEY_ID);
        }
    });

    // 获取屏幕信息
    let screens = Screen::all().unwrap();
    let primary_screen = screens.first().unwrap();
    let screen_width = primary_screen.display_info.width as usize;
    let screen_height = primary_screen.display_info.height as usize;

    println!("Primary screen: {}x{}", screen_width, screen_height);
    println!("Press Ctrl+Alt+D to capture screen, ESC to exit");
    println!("Press Ctrl+S to save selected region");

    // 创建窗口选项 - 设置为无边框全屏且置顶
    let mut window_options = WindowOptions::default();
    window_options.resize = false;
    window_options.scale = Scale::X1;
    window_options.scale_mode = ScaleMode::AspectRatioStretch;
    window_options.topmost = true;
    window_options.borderless = true;
    window_options.transparency = true;

    // 创建窗口
    let mut window = Window::new(
        "Screen Capture - Press ESC to exit",
        screen_width,
        screen_height,
        window_options,
    )
    .unwrap_or_else(|e| {
        panic!("{}", e);
    });

    // 无边框代码，确保窗口无边框
    #[cfg(windows)]
    unsafe {
        use winapi::um::winuser::{GWL_STYLE, SetWindowLongPtrW};
        use winapi::um::winuser::{WS_POPUP, WS_VISIBLE};

        let hwnd = window.get_window_handle() as *mut _;
        // 设置窗口样式为无边框
        SetWindowLongPtrW(hwnd, GWL_STYLE, (WS_POPUP | WS_VISIBLE) as isize);
    }

    // 设置帧率限制
    window.set_target_fps(120);

    // 初始化状态
    let mut state = State::Idle;
    let mut mouse_pressed = false;

    // 事件队列
    let mut events = VecDeque::new();

    // 初始时隐藏窗口 - 使用更可靠的方法
    window.set_position(-(screen_width as isize * 2), -(screen_height as isize * 2));

    // 按键状态跟踪
    let mut key_states = std::collections::HashMap::new();

    // 主事件循环
    while window.is_open() {
        // 检查来自消息线程的事件
        while let Ok(event) = rx.try_recv() {
            events.push_back(event);
        }

        // 处理窗口事件
        window.get_keys().iter().for_each(|key| {
            if !key_states.contains_key(key) {
                key_states.insert(*key, true);
                events.push_back(AppEvent::KeyPressed(*key));
            }
        });

        // 检查释放的键
        let current_keys: Vec<Key> = window.get_keys().to_vec();
        let previous_keys: Vec<Key> = key_states.keys().cloned().collect();

        for key in previous_keys {
            if !current_keys.contains(&key) {
                key_states.remove(&key);
                events.push_back(AppEvent::KeyReleased(key));
            }
        }

        // 处理鼠标事件
        if let Some((x, y)) = window.get_mouse_pos(MouseMode::Clamp) {
            events.push_back(AppEvent::MouseMoved(x, y));

            if window.get_mouse_down(MouseButton::Left) && !mouse_pressed {
                mouse_pressed = true;
                events.push_back(AppEvent::MousePressed(MouseButton::Left, x, y));
            } else if !window.get_mouse_down(MouseButton::Left) && mouse_pressed {
                mouse_pressed = false;
                events.push_back(AppEvent::MouseReleased(MouseButton::Left, x, y));
            }
        }

        // 处理所有事件
        let mut processed_events = Vec::new();
        while let Some(event) = events.pop_front() {
            // 如果是退出事件，退出程序
            if let AppEvent::Quit = event {
                break;
            }

            let new_state = handle_event(event.clone(), &state, &mut window, primary_screen);
            processed_events.push((event, new_state));
        }

        // 应用状态变化（如果有）
        for (event, new_state) in processed_events {
            if let Some(new_state) = new_state {
                state = new_state;
            }
        }

        // 根据当前状态更新显示
        update_display(&mut window, &state);

        // 更新窗口
        window.update();

        // 短暂延迟以减少CPU使用
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    // 发送退出消息给消息线程
    unsafe {
        PostQuitMessage(0);
    }
}

// 事件处理函数 - 现在返回Option<State>，表示可能的状态变化
fn handle_event(
    event: AppEvent,
    state: &State,
    window: &mut Window,
    primary_screen: &Screen,
) -> Option<State> {
    match (event, state) {
        (AppEvent::KeyPressed(Key::Escape), State::Idle) => {
            // Idle状态下按ESC退出程序
            std::process::exit(0);
        }
        (AppEvent::KeyPressed(Key::Escape), State::FullscreenCapture(img)) => {
            // 截图后按ESC回到Idle状态
            window.set_position(-(img.width() as isize * 2), -(img.height() as isize * 2));
            window.set_title("Screen Capture - Press Ctrl+Alt+D to capture screen, ESC to exit");
            Some(State::Idle)
        }
        (AppEvent::KeyPressed(Key::Escape), State::SelectingRegion(img, _, _)) => {
            // 选择区域时按ESC回到FullscreenCapture状态
            window.set_title("Screen captured - Click and drag to select region, ESC to cancel");
            Some(State::FullscreenCapture(img.clone()))
        }
        (AppEvent::KeyPressed(Key::Escape), State::RegionSelected(img, _)) => {
            // 区域选择后按ESC回到FullscreenCapture状态
            window.set_title("Screen captured - Click and drag to select region, ESC to cancel");
            Some(State::FullscreenCapture(img.clone()))
        }
        (AppEvent::GlobalHotkeyPressed, State::Idle) => {
            // 全局热键触发截图
            // 最小化窗口
            window.set_position(
                -(primary_screen.display_info.width as isize * 2),
                -(primary_screen.display_info.height as isize * 2),
            );

            // 短暂延迟确保窗口已最小化
            std::thread::sleep(std::time::Duration::from_millis(100));

            // 捕获屏幕
            match capture_screen(primary_screen) {
                Ok(image_buffer) => {
                    // 恢复窗口位置到全屏
                    window.set_position(0, 0);
                    window.set_title(
                        "Screen captured - Click and drag to select region, ESC to cancel",
                    );
                    Some(State::FullscreenCapture(image_buffer))
                }
                Err(e) => {
                    // 恢复窗口位置
                    window.set_position(0, 0);
                    eprintln!("Failed to capture screen: {}", e);
                    Some(State::Idle)
                }
            }
        }
        (AppEvent::KeyPressed(Key::S), State::RegionSelected(img, region)) => {
            // 保存选择的区域
            save_image(
                &img,
                region.0,
                region.1,
                region.2 as u32,
                region.3 as u32,
                primary_screen.display_info.width as u32,
                primary_screen.display_info.height as u32,
                None,
            );

            // 保存后隐藏窗口并回到Idle状态
            window.set_position(-(img.width() as isize * 2), -(img.height() as isize * 2));
            window.set_title("Screen Capture - Press Ctrl+Alt+D to capture screen, ESC to exit");
            Some(State::Idle)
        }
        (AppEvent::MousePressed(MouseButton::Left, x, y), State::FullscreenCapture(img)) => {
            // 开始选择区域
            Some(State::SelectingRegion(
                img.clone(),
                (x as i32, y as i32),
                (x as i32, y as i32),
            ))
        }
        (AppEvent::MouseMoved(x, y), State::SelectingRegion(img, start, _)) => {
            // 更新选择区域
            Some(State::SelectingRegion(
                img.clone(),
                *start,
                (x as i32, y as i32),
            ))
        }
        (
            AppEvent::MouseReleased(MouseButton::Left, x, y),
            State::SelectingRegion(img, start, current),
        ) => {
            let width = (current.0 - start.0).abs() as u32;
            let height = (current.1 - start.1).abs() as u32;

            if width > 10 && height > 10 {
                let region = (
                    start.0.min(current.0),
                    start.1.min(current.1),
                    width as i32,
                    height as i32,
                );

                window.set_title("Region selected - Press Ctrl+S to save, ESC to re-select");
                Some(State::RegionSelected(img.clone(), region))
            } else {
                // 区域太小，继续选择
                window
                    .set_title("Screen captured - Click and drag to select region, ESC to cancel");
                Some(State::FullscreenCapture(img.clone()))
            }
        }
        // 默认情况：不改变状态
        _ => None,
    }
}

// 更新显示函数
fn update_display(window: &mut Window, state: &State) {
    match state {
        State::Idle => {
            // 空闲状态，无需显示
        }
        State::FullscreenCapture(image) => {
            display_image(window, image, None);
        }
        State::SelectingRegion(image, start, current) => {
            let region = Some((
                start.0.min(current.0),
                start.1.min(current.1),
                (current.0 - start.0).abs(),
                (current.1 - start.1).abs(),
            ));
            display_image(window, image, region);
        }
        State::RegionSelected(image, region) => {
            display_image(window, image, Some(*region));
        }
    }
}

// 捕获屏幕函数
fn capture_screen(
    screen: &Screen,
) -> Result<ImageBuffer<Rgba<u8>, Vec<u8>>, Box<dyn std::error::Error>> {
    let screenshot = screen.capture()?;
    let width = screenshot.width() as u32;
    let height = screenshot.height() as u32;
    let buffer = screenshot.to_vec();

    Ok(ImageBuffer::from_vec(width, height, buffer).unwrap())
}

// 显示图像函数
fn display_image(
    window: &mut Window,
    image: &ImageBuffer<Rgba<u8>, Vec<u8>>,
    region: Option<(i32, i32, i32, i32)>,
) {
    let (width, height) = image.dimensions();
    let mut buffer = vec![0; (width * height) as usize];

    // 将图像数据复制到缓冲区
    for (i, pixel) in image.pixels().enumerate() {
        let r = pixel[0] as u32;
        let g = pixel[1] as u32;
        let b = pixel[2] as u32;
        let a = pixel[3] as u32;

        buffer[i] = (a << 24) | (r << 16) | (g << 8) | b;
    }

    // 如果有选择区域，变灰区域外的部分
    if let Some((x, y, w, h)) = region {
        for i in 0..(width as usize) {
            for j in 0..(height as usize) {
                let idx = j * width as usize + i;
                if i < x as usize
                    || i >= (x + w) as usize
                    || j < y as usize
                    || j >= (y + h) as usize
                {
                    // 区域外变灰
                    let pixel = buffer[idx];
                    let r = ((pixel >> 16) & 0xFF) as f32;
                    let g = ((pixel >> 8) & 0xFF) as f32;
                    let b = (pixel & 0xFF) as f32;
                    let gray = (r * 0.3 + g * 0.59 + b * 0.11) as u32;
                    buffer[idx] = (0x80 << 24) | (gray << 16) | (gray << 8) | gray;
                }
            }
        }

        // 绘制选择框
        let x1 = x as usize;
        let y1 = y as usize;
        let x2 = (x + w) as usize;
        let y2 = (y + h) as usize;

        for i in x1..=x2 {
            if i < width as usize {
                if y1 < height as usize {
                    buffer[y1 * width as usize + i] = 0xFFFF0000; // 红色边框
                }
                if y2 < height as usize {
                    buffer[y2 * width as usize + i] = 0xFFFF0000;
                }
            }
        }

        for j in y1..=y2 {
            if j < height as usize {
                if x1 < width as usize {
                    buffer[j * width as usize + x1] = 0xFFFF0000;
                }
                if x2 < width as usize {
                    buffer[j * width as usize + x2] = 0xFFFF0000;
                }
            }
        }
    }

    window
        .update_with_buffer(&buffer, width as usize, height as usize)
        .unwrap();
}

// 保存图像函数
fn save_image(
    image: &ImageBuffer<Rgba<u8>, Vec<u8>>,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    screen_width: u32,
    screen_height: u32,
    sub_region: Option<(i32, i32, u32, u32)>,
) {
    // 创建目录
    let dir_name = format!("W{}H{}", screen_width, screen_height);
    let _ = std::fs::create_dir_all(&dir_name);

    // 生成文件名
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis();

    let mut file_name = format!(
        "{}/screenshot_{}_Lx{}Ty{}W{}H{}",
        dir_name, timestamp, x, y, width, height
    );

    if let Some((sx, sy, sw, sh)) = sub_region {
        file_name.push_str(&format!("_Sx{}Sy{}Sw{}Sh{}", sx, sy, sw, sh));
    }

    file_name.push_str(".bmp");

    // 裁剪图像
    let cropped = image::imageops::crop_imm(image, x as u32, y as u32, width, height).to_image();

    // 保存图像
    if let Err(e) = cropped.save(&file_name) {
        eprintln!("Failed to save image: {}", e);
    } else {
        println!("Image saved as: {}", file_name);
    }
}