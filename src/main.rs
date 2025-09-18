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
use webp::{Encoder, WebPMemory};
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
    GlobalHotkeyPressed,
    Quit,
}

// 缓存的显示数据
#[derive(Clone)]
struct DisplayCache {
    original_buffer: Vec<u32>,  // 原始图像的ARGB缓冲区
    dimmed_buffer: Vec<u32>,     // 灰度化后的缓冲区
    display_buffer: Vec<u32>,    // 实际显示的缓冲区
    width: u32,
    height: u32,
}

impl DisplayCache {
    fn new(image: &ImageBuffer<Rgba<u8>, Vec<u8>>) -> Self {
        let (width, height) = image.dimensions();
        let size = (width * height) as usize;

        let mut original_buffer = vec![0u32; size];
        let mut dimmed_buffer = vec![0u32; size];

        // 预计算原始图像和灰度图像
        for (i, pixel) in image.pixels().enumerate() {
            let r = pixel[0] as u32;
            let g = pixel[1] as u32;
            let b = pixel[2] as u32;
            let a = pixel[3] as u32;

            // 原始颜色
            original_buffer[i] = (a << 24) | (r << 16) | (g << 8) | b;

            // 灰度化：保留原始颜色但降低亮度和饱和度
            let gray = ((r * 3 + g * 6 + b * 1) / 10) as u32;
            let dimmed_r = (r * 3 + gray * 7) / 10;
            let dimmed_g = (g * 3 + gray * 7) / 10;
            let dimmed_b = (b * 3 + gray * 7) / 10;
            dimmed_buffer[i] = (a << 24) | (dimmed_r << 16) | (dimmed_g << 8) | dimmed_b;
        }

        let display_buffer = original_buffer.clone();

        Self {
            original_buffer,
            dimmed_buffer,
            display_buffer,
            width,
            height,
        }
    }

    fn update_display(&mut self, red_region: Option<(i32, i32, i32, i32)>, green_region: Option<(i32, i32, i32, i32)>) {
        if let Some((rx, ry, rw, rh)) = red_region {
            // 先复制灰度背景
            self.display_buffer.copy_from_slice(&self.dimmed_buffer);

            // 恢复红框内的原始图像
            for y in ry.max(0)..(ry + rh).min(self.height as i32) {
                let y_offset = y as usize * self.width as usize;
                let start_x = rx.max(0) as usize;
                let end_x = (rx + rw).min(self.width as i32) as usize;

                for x in start_x..end_x {
                    let idx = y_offset + x;
                    self.display_buffer[idx] = self.original_buffer[idx];
                }
            }

            // 绘制红框
            self.draw_rectangle((rx, ry, rw, rh), 0xFFFF0000);

            // 绘制绿框（如果有）
            if let Some(green) = green_region {
                self.draw_rectangle(green, 0xFF00FF00);
            }
        } else {
            // 没有选择区域时显示原始图像
            self.display_buffer.copy_from_slice(&self.original_buffer);
        }
    }

    fn draw_rectangle(&mut self, rect: (i32, i32, i32, i32), color: u32) {
        let (x, y, w, h) = rect;
        let width = self.width as i32;
        let height = self.height as i32;

        // 绘制上下边框
        for i in x.max(0)..(x + w).min(width) {
            if y >= 0 && y < height {
                self.display_buffer[y as usize * self.width as usize + i as usize] = color;
            }
            if (y + h - 1) >= 0 && (y + h - 1) < height {
                self.display_buffer[(y + h - 1) as usize * self.width as usize + i as usize] = color;
            }
        }

        // 绘制左右边框
        for j in y.max(0)..(y + h).min(height) {
            if x >= 0 && x < width {
                self.display_buffer[j as usize * self.width as usize + x as usize] = color;
            }
            if (x + w - 1) >= 0 && (x + w - 1) < width {
                self.display_buffer[j as usize * self.width as usize + (x + w - 1) as usize] = color;
            }
        }
    }
}

// 程序状态
enum State {
    Idle,
    FullscreenCapture(ImageBuffer<Rgba<u8>, Vec<u8>>, DisplayCache),
    SelectingRegion(ImageBuffer<Rgba<u8>, Vec<u8>>, DisplayCache, (i32, i32), (i32, i32)),
    RegionSelected(ImageBuffer<Rgba<u8>, Vec<u8>>, DisplayCache, (i32, i32, i32, i32)),
    SelectingSubRegion(
        ImageBuffer<Rgba<u8>, Vec<u8>>,
        DisplayCache,
        (i32, i32, i32, i32),
        (i32, i32),
        (i32, i32),
    ),
    SubRegionSelected(
        ImageBuffer<Rgba<u8>, Vec<u8>>,
        DisplayCache,
        (i32, i32, i32, i32),
        (i32, i32, i32, i32),
    ),
}

// 全局热键ID
const HOTKEY_ID: i32 = 1;
const SAVE_HOTKEY_ID: i32 = 2;

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
            RegisterHotKey(null_mut(), SAVE_HOTKEY_ID, MOD_CONTROL as u32, 'S' as u32);
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
                WM_HOTKEY => match msg.wParam as i32 {
                    HOTKEY_ID => {
                        tx_clone.send(AppEvent::GlobalHotkeyPressed).unwrap();
                    }
                    SAVE_HOTKEY_ID => {
                        tx_clone.send(AppEvent::KeyPressed(Key::S)).unwrap();
                    }
                    _ => {}
                },
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

    // 创建窗口选项
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

    // 无边框代码
    #[cfg(windows)]
    unsafe {
        use winapi::um::winuser::{GWL_STYLE, SetWindowLongPtrW};
        use winapi::um::winuser::{WS_POPUP, WS_VISIBLE};

        let hwnd = window.get_window_handle() as *mut _;
        SetWindowLongPtrW(hwnd, GWL_STYLE, (WS_POPUP | WS_VISIBLE) as isize);
    }

    // 设置帧率限制
    window.set_target_fps(60); // 降低帧率以减少CPU使用

    // 初始化状态
    let mut state = State::Idle;
    let mut mouse_pressed = false;

    // 事件队列
    let mut events = VecDeque::new();

    // 初始时隐藏窗口
    window.set_position(-(screen_width as isize * 2), -(screen_height as isize * 2));

    // 按键状态跟踪
    let mut key_states = std::collections::HashMap::new();

    // 缓存图像显示缓冲区，避免频繁分配内存
    let mut display_buffer: Option<Vec<u32>> = None;

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
            if let AppEvent::Quit = event {
                break;
            }

            let new_state = handle_event(event.clone(), &state, &mut window, primary_screen);
            processed_events.push((event, new_state));
        }

        // 应用状态变化（如果有）
        for (_event, new_state) in processed_events {
            if let Some(new_state) = new_state {
                state = new_state;
                // 状态改变时重置显示缓冲区
                display_buffer = None;
            }
        }

        // 根据当前状态更新显示
        update_display(&mut window, &mut state, &mut display_buffer);

        // 更新窗口
        window.update();

        // 短暂延迟以减少CPU使用
        std::thread::sleep(std::time::Duration::from_millis(16)); // ~60fps
    }

    // 发送退出消息给消息线程
    unsafe {
        PostQuitMessage(0);
    }
}

// 事件处理函数
fn handle_event(
    event: AppEvent,
    state: &State,
    window: &mut Window,
    primary_screen: &Screen,
) -> Option<State> {
    match (event, state) {
        (AppEvent::KeyPressed(Key::Escape), State::Idle) => {
            std::process::exit(0);
        }
        (AppEvent::KeyPressed(Key::Escape), State::FullscreenCapture(img, _)) => {
            window.set_position(-(img.width() as isize * 2), -(img.height() as isize * 2));
            window.set_title("Screen Capture - Press Ctrl+Alt+D to capture screen, ESC to exit");
            Some(State::Idle)
        }
        (AppEvent::KeyPressed(Key::Escape), State::SelectingRegion(img, cache, _, _)) => {
            window.set_title("Screen captured - Click and drag to select region, ESC to cancel");
            Some(State::FullscreenCapture(img.clone(), cache.clone()))
        }
        (AppEvent::KeyPressed(Key::Escape), State::RegionSelected(img, cache, _region)) => {
            window.set_title("Screen captured - Click and drag to select region, ESC to cancel");
            Some(State::FullscreenCapture(img.clone(), cache.clone()))
        }
        (AppEvent::KeyPressed(Key::Escape), State::SelectingSubRegion(img, cache, red_region, _, _)) => {
            window.set_title("Region selected - Press Ctrl+S to save, or click and drag to select sub-region, ESC to re-select");
            Some(State::RegionSelected(img.clone(), cache.clone(), *red_region))
        }
        (AppEvent::KeyPressed(Key::Escape), State::SubRegionSelected(img, cache, red_region, _)) => {
            window.set_title("Region selected - Press Ctrl+S to save, or click and drag to select sub-region, ESC to re-select");
            Some(State::RegionSelected(img.clone(), cache.clone(), *red_region))
        }
        (AppEvent::GlobalHotkeyPressed, State::Idle) => {
            window.set_position(
                -(primary_screen.display_info.width as isize * 2),
                -(primary_screen.display_info.height as isize * 2),
            );

            std::thread::sleep(std::time::Duration::from_millis(100));

            match capture_screen(primary_screen) {
                Ok(image_buffer) => {
                    window.set_position(0, 0);
                    window.set_title(
                        "Screen captured - Click and drag to select region, ESC to cancel",
                    );
                    let cache = DisplayCache::new(&image_buffer);
                    Some(State::FullscreenCapture(image_buffer, cache))
                }
                Err(e) => {
                    window.set_position(0, 0);
                    eprintln!("Failed to capture screen: {}", e);
                    Some(State::Idle)
                }
            }
        }
        (AppEvent::KeyPressed(Key::S), State::RegionSelected(img, _, region)) => {
            save_image_webp(
                &img,
                region.0,
                region.1,
                region.2 as u32,
                region.3 as u32,
                primary_screen.display_info.width as u32,
                primary_screen.display_info.height as u32,
                None,
            );

            window.set_position(-(img.width() as isize * 2), -(img.height() as isize * 2));
            window.set_title("Screen Capture - Press Ctrl+Alt+D to capture screen, ESC to exit");
            Some(State::Idle)
        }
        (AppEvent::KeyPressed(Key::S), State::SubRegionSelected(img, _, red_region, green_region)) => {
            save_image_webp(
                &img,
                red_region.0,
                red_region.1,
                red_region.2 as u32,
                red_region.3 as u32,
                primary_screen.display_info.width as u32,
                primary_screen.display_info.height as u32,
                Some((
                    green_region.0,
                    green_region.1,
                    green_region.2 as u32,
                    green_region.3 as u32,
                )),
            );

            window.set_position(-(img.width() as isize * 2), -(img.height() as isize * 2));
            window.set_title("Screen Capture - Press Ctrl+Alt+D to capture screen, ESC to exit");
            Some(State::Idle)
        }
        (AppEvent::MousePressed(MouseButton::Left, x, y), State::FullscreenCapture(img, cache)) => Some(
            State::SelectingRegion(img.clone(), cache.clone(), (x as i32, y as i32), (x as i32, y as i32)),
        ),
        (AppEvent::MouseMoved(x, y), State::SelectingRegion(img, cache, start, _)) => Some(
            State::SelectingRegion(img.clone(), cache.clone(), *start, (x as i32, y as i32)),
        ),
        (
            AppEvent::MouseReleased(MouseButton::Left, _x, _y),
            State::SelectingRegion(img, cache, start, current),
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

                window.set_title("Region selected - Press Ctrl+S to save, or click and drag to select sub-region, ESC to re-select");
                Some(State::RegionSelected(img.clone(), cache.clone(), region))
            } else {
                window
                    .set_title("Screen captured - Click and drag to select region, ESC to cancel");
                Some(State::FullscreenCapture(img.clone(), cache.clone()))
            }
        }
        (AppEvent::MousePressed(MouseButton::Left, x, y), State::RegionSelected(img, cache, region)) => {
            // 检查点击是否在红框内
            if x as i32 >= region.0
                && x as i32 <= region.0 + region.2
                && y as i32 >= region.1
                && y as i32 <= region.1 + region.3
            {
                Some(State::SelectingSubRegion(
                    img.clone(),
                    cache.clone(),
                    *region,
                    (x as i32, y as i32),
                    (x as i32, y as i32),
                ))
            } else {
                None // 点击在红框外，不处理
            }
        }
        (AppEvent::MouseMoved(x, y), State::SelectingSubRegion(img, cache, red_region, start, _)) => {
            // 限制绿框在红框内
            let clamped_x = x.clamp(
                red_region.0 as f32,
                red_region.0 as f32 + red_region.2 as f32,
            );
            let clamped_y = y.clamp(
                red_region.1 as f32,
                red_region.1 as f32 + red_region.3 as f32,
            );

            Some(State::SelectingSubRegion(
                img.clone(),
                cache.clone(),
                *red_region,
                *start,
                (clamped_x as i32, clamped_y as i32),
            ))
        }
        (
            AppEvent::MouseReleased(MouseButton::Left, _x, _y),
            State::SelectingSubRegion(img, cache, red_region, start, current),
        ) => {
            let width = (current.0 - start.0).abs() as u32;
            let height = (current.1 - start.1).abs() as u32;

            if width > 5 && height > 5 {
                let green_region = (
                    start.0.min(current.0),
                    start.1.min(current.1),
                    width as i32,
                    height as i32,
                );

                window.set_title("Sub-region selected - Press Ctrl+S to save, ESC to re-select");
                Some(State::SubRegionSelected(
                    img.clone(),
                    cache.clone(),
                    *red_region,
                    green_region,
                ))
            } else {
                window.set_title("Region selected - Press Ctrl+S to save, or click and drag to select sub-region, ESC to re-select");
                Some(State::RegionSelected(img.clone(), cache.clone(), *red_region))
            }
        }
        // 默认情况：不改变状态
        _ => None,
    }
}

// 更新显示函数
fn update_display(window: &mut Window, state: &mut State, _display_buffer: &mut Option<Vec<u32>>) {
    match state {
        State::Idle => {
            // 空闲状态，无需显示
        }
        State::FullscreenCapture(_, cache) => {
            cache.update_display(None, None);
            window.update_with_buffer(&cache.display_buffer, cache.width as usize, cache.height as usize).unwrap();
        }
        State::SelectingRegion(_, cache, start, current) => {
            let region = Some((
                start.0.min(current.0),
                start.1.min(current.1),
                (current.0 - start.0).abs(),
                (current.1 - start.1).abs(),
            ));
            cache.update_display(region, None);
            window.update_with_buffer(&cache.display_buffer, cache.width as usize, cache.height as usize).unwrap();
        }
        State::RegionSelected(_, cache, region) => {
            cache.update_display(Some(*region), None);
            window.update_with_buffer(&cache.display_buffer, cache.width as usize, cache.height as usize).unwrap();
        }
        State::SelectingSubRegion(_, cache, red_region, start, current) => {
            let green_region = Some((
                start.0.min(current.0),
                start.1.min(current.1),
                (current.0 - start.0).abs(),
                (current.1 - start.1).abs(),
            ));
            cache.update_display(Some(*red_region), green_region);
            window.update_with_buffer(&cache.display_buffer, cache.width as usize, cache.height as usize).unwrap();
        }
        State::SubRegionSelected(_, cache, red_region, green_region) => {
            cache.update_display(Some(*red_region), Some(*green_region));
            window.update_with_buffer(&cache.display_buffer, cache.width as usize, cache.height as usize).unwrap();
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


// 保存为WebP格式的函数（无损）
fn save_image_webp(
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

    //// 添加子框信息，暂不使用
    // if let Some((sx, sy, sw, sh)) = sub_region {
    //     file_name.push_str(&format!("_Sx{}Sy{}Sw{}Sh{}", sx, sy, sw, sh));
    // }

    file_name.push_str(".webp");

    // 裁剪图像
    let cropped = if let Some((sx, sy, sw, sh)) = sub_region {
        // 保存绿框内的图像
        image::imageops::crop_imm(image, sx as u32, sy as u32, sw, sh).to_image()
    } else {
        // 保存红框内的图像
        image::imageops::crop_imm(image, x as u32, y as u32, width, height).to_image()
    };

    // 转换为WebP格式（无损）
    let encoder = Encoder::from_rgba(cropped.as_raw(), cropped.width(), cropped.height());
    let webp_data: WebPMemory = encoder.encode_lossless();

    // 保存图像
    if let Err(e) = std::fs::write(&file_name, webp_data.as_ref()) {
        eprintln!("Failed to save image: {}", e);
    } else {
        println!("Image saved as: {}", file_name);
    }
}
