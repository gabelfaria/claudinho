#![windows_subsystem = "windows"]
//! Vigia residente do Claudinho: Win32 puro, sem GPU (meta: < 30 MB parado).
//! - Bolha flutuante com o Claude sorrindo (estilo iPhone): arrastável, gruda na borda, clique abre a conversa.
//! - Win + Espaço + C abre/fecha a janela de conversa (claudinho-ui.exe).
//! - Ícone na bandeja (clique abre; botão direito: Abrir / Sair).
//! - A cada 20 s: lembretes vencidos. De hora em hora (9h–22h): verificação com o Haiku.
//! - Em jogo/tela cheia: bolha some, nada abre por cima; lembrete vira notificação.

#[path = "../dia_comum.rs"]
mod comum;
use comum::*;

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
use windows_sys::Win32::UI::Shell::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn CreateMutexW(a: *const core::ffi::c_void, dono: i32, nome: *const u16) -> isize;
}

const WM_TRAY: u32 = WM_APP + 1;
const WM_ATALHO: u32 = WM_APP + 2;
const WM_RESULTADO: u32 = WM_APP + 3;
const ID_ABRIR: usize = 1;
const ID_SAIR: usize = 2;
const ID_HISTORICO: usize = 4;
const ID_MEMORIA: usize = 5;
const TIMER_TIQUE: usize = 1;
const TIMER_BOLHA: usize = 2;

const BOLHA: i32 = 64; // lado da janela da bolha (px)
const MASCOTE: u32 = 54; // lado do mascote dentro dela

static JANELA: AtomicIsize = AtomicIsize::new(0);
static BOLHA_H: AtomicIsize = AtomicIsize::new(0);
static GANCHO_MOUSE: AtomicIsize = AtomicIsize::new(0);
static COMPACTANDO: AtomicBool = AtomicBool::new(false);
/// a animação de abrir termina mostrando o chat pré-carregado
static REVELAR: AtomicBool = AtomicBool::new(false);
const WM_BOLHA_SOLTOU: u32 = WM_APP + 10;
const WM_BOLHA_MENU: u32 = WM_APP + 11;
static OCUPADO: AtomicBool = AtomicBool::new(false);
static ALERTAS: Mutex<Vec<String>> = Mutex::new(Vec::new());
static ULTIMA_HORA: Mutex<String> = Mutex::new(String::new());

struct EstadoBolha {
    dc: HDC,
    visivel: bool,
    hover: bool,
    arrastando: bool,
    moveu: bool,
    inicio_cursor: POINT,
    inicio_janela: POINT,
    aviso: bool, // alerta pendente (pontinho)
}
static mut BOLHA_EST: Option<EstadoBolha> = None;
#[allow(static_mut_refs)]
fn est() -> &'static mut EstadoBolha {
    unsafe { BOLHA_EST.as_mut().unwrap() }
}

fn jogando() -> bool {
    let mut st: QUERY_USER_NOTIFICATION_STATE = 0;
    unsafe {
        if SHQueryUserNotificationState(&mut st) == 0 {
            return st == QUNS_BUSY || st == QUNS_RUNNING_D3D_FULL_SCREEN || st == QUNS_PRESENTATION_MODE;
        }
    }
    false
}

fn janela_ui() -> HWND {
    unsafe { FindWindowW(std::ptr::null(), wide(TITULO_UI).as_ptr()) }
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn OpenProcess(acesso: u32, herdar: i32, pid: u32) -> HANDLE;
    fn SetProcessWorkingSetSize(h: HANDLE, min: usize, max: usize) -> i32;
    fn GetCurrentThreadId() -> u32;
}
#[link(name = "user32")]
unsafe extern "system" {
    fn AttachThreadInput(de: u32, para: u32, ligar: i32) -> i32;
}

fn chat_visivel() -> bool {
    let h = janela_ui();
    !h.is_null() && unsafe { IsWindowVisible(h) } != 0
}
fn chat_escondido() -> bool {
    let h = janela_ui();
    !h.is_null() && unsafe { IsWindowVisible(h) } == 0
}

/// Esvazia o working set do chat escondido: a RAM volta para o Windows; abrir de novo é instantâneo.
fn aparar_ui() {
    unsafe {
        let mut pid = 0u32;
        GetWindowThreadProcessId(janela_ui(), &mut pid);
        if pid == 0 {
            return;
        }
        let p = OpenProcess(0x0100 | 0x1000, 0, pid); // SET_QUOTA | QUERY_LIMITED_INFORMATION
        if !p.is_null() {
            SetProcessWorkingSetSize(p, usize::MAX, usize::MAX);
            CloseHandle(p);
        }
    }
}

/// Deixa o chat carregado e escondido, pronto para abrir sem espera.
fn pre_carregar_ui() {
    if janela_ui().is_null() {
        let _ = std::process::Command::new(exe_irmao("claudinho-ui.exe")).arg("--oculta").spawn();
    }
}

/// Traz a janela para frente mesmo vindo de um processo em segundo plano.
fn forcar_foco(h: HWND) {
    unsafe {
        let fg = GetForegroundWindow();
        let t_fg = GetWindowThreadProcessId(fg, std::ptr::null_mut());
        let eu = GetCurrentThreadId();
        let ligado = t_fg != 0 && t_fg != eu && AttachThreadInput(eu, t_fg, 1) != 0;
        SetForegroundWindow(h);
        BringWindowToTop(h);
        if ligado {
            AttachThreadInput(eu, t_fg, 0);
        }
    }
}

fn abrir_ui(piscar: bool) {
    est().aviso = false;
    let h = janela_ui();
    if !h.is_null() {
        unsafe {
            if IsWindowVisible(h) != 0 {
                forcar_foco(h);
                return;
            }
            // pré-carregado: posiciona já, a animação revela no fim
            let de = rect_bolha();
            let para = rect_chat(de);
            SetWindowPos(h, std::ptr::null_mut(), para.left, para.top, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
            mostrar_bolha(false);
            iniciar_anim(de, para, true);
            REVELAR.store(true, Ordering::Relaxed);
        }
        return;
    }
    let mut c = std::process::Command::new(exe_irmao("claudinho-ui.exe"));
    if piscar {
        c.arg("--piscar");
    }
    let hb = BOLHA_H.load(Ordering::Relaxed) as HWND;
    if !hb.is_null() {
        let mut r: RECT = unsafe { std::mem::zeroed() };
        unsafe { GetWindowRect(hb, &mut r) };
        c.arg("--perto").arg(format!("{},{},{},{}", r.left, r.top, r.right - r.left, r.bottom - r.top));
    }
    let _ = c.spawn();
    let de = rect_bolha();
    mostrar_bolha(false);
    let para = rect_chat(de);
    iniciar_anim(de, para, true);
}

// ---------- animação expandir/recolher ----------
const ANIM_LARG: f32 = 400.0;
const ANIM_ALT: f32 = 560.0;
const ANIM_MARGEM: f32 = 18.0;
const TIMER_ANIM: usize = 3;
const WM_UI_FECHOU: u32 = WM_APP + 12;
const WM_ABRIR: u32 = WM_APP + 13;
static ANIM_H: AtomicIsize = AtomicIsize::new(0);

struct Anim {
    de: RECT,
    para: RECT,
    inicio: std::time::Instant,
    abrindo: bool,
}
static mut ANIM: Option<Anim> = None;

#[link(name = "user32")]
unsafe extern "system" {
    fn GetDpiForSystem() -> u32;
}

fn rect_bolha() -> RECT {
    let mut r: RECT = unsafe { std::mem::zeroed() };
    unsafe { GetWindowRect(BOLHA_H.load(Ordering::Relaxed) as HWND, &mut r) };
    r
}

/// Mesma regra da janela do chat: canto superior esquerdo no da bolha, encaixado na área de trabalho.
fn rect_chat(b: RECT) -> RECT {
    let a = area();
    let esc = unsafe { GetDpiForSystem() }.max(96) as f32 / 96.0;
    let (w, h, m) = ((ANIM_LARG * esc) as i32, (ANIM_ALT * esc) as i32, (ANIM_MARGEM * esc) as i32);
    let x = b.left.min(a.right - w - m).max(a.left + m);
    let y = b.top.min(a.bottom - h - m).max(a.top + m);
    RECT { left: x, top: y, right: x + w, bottom: y + h }
}

fn animando() -> bool {
    #[allow(static_mut_refs)]
    unsafe {
        ANIM.is_some()
    }
}

fn iniciar_anim(de: RECT, para: RECT, abrindo: bool) {
    unsafe {
        ANIM = Some(Anim { de, para, inicio: std::time::Instant::now(), abrindo });
        quadro_anim();
        SetTimer(JANELA.load(Ordering::Relaxed) as HWND, TIMER_ANIM, 10, None);
    }
}

const ANIM_MS: f32 = 160.0;

fn quadro_anim() {
    #[allow(static_mut_refs)]
    let Some(a) = (unsafe { ANIM.as_ref() }) else { return };
    let ms = a.inicio.elapsed().as_secs_f32() * 1000.0;
    let t = (ms / ANIM_MS).min(1.0);
    let ah = ANIM_H.load(Ordering::Relaxed) as HWND;
    if t >= 1.0 {
        if a.abrindo && REVELAR.swap(false, Ordering::Relaxed) {
            let ui = janela_ui();
            if !ui.is_null() {
                unsafe { ShowWindow(ui, SW_SHOWNOACTIVATE) };
                forcar_foco(ui);
            }
        }
        // abrindo: segura o último quadro até o chat aparecer (no máximo 1,5 s)
        let ui = janela_ui();
        let pronto = !a.abrindo || (!ui.is_null() && unsafe { IsWindowVisible(ui) } != 0) || ms > 1500.0;
        if pronto {
            let abrindo = a.abrindo;
            unsafe {
                KillTimer(JANELA.load(Ordering::Relaxed) as HWND, TIMER_ANIM);
                if !abrindo {
                    mostrar_bolha(!jogando()); // a bolha surge antes do fantasma sumir
                    aparar_ui();
                }
                ShowWindow(ah, SW_HIDE);
                ANIM = None;
            }
            return;
        }
    }
    let e = 1.0 - (1.0 - t).powi(3); // desacelera no fim
    let lerp = |x: i32, y: i32| x + ((y - x) as f32 * e).round() as i32;
    let r = RECT {
        left: lerp(a.de.left, a.para.left),
        top: lerp(a.de.top, a.para.top),
        right: lerp(a.de.right, a.para.right),
        bottom: lerp(a.de.bottom, a.para.bottom),
    };
    // transparência acompanha: mais leve perto da bolha
    let opac = if a.abrindo { 0.55 + 0.45 * e } else { 1.0 - 0.45 * e };
    pintar_anim(ah, r, opac);
}

fn pintar_anim(h: HWND, r: RECT, opac: f32) {
    let (w, hh) = ((r.right - r.left).max(2), (r.bottom - r.top).max(2));
    unsafe {
        let (bmp, bits) = dib(w, hh);
        let raio = 12.0f32.min(w as f32 / 2.0).min(hh as f32 / 2.0);
        let (bg, borda) = ([0xF5u8, 0xF3, 0xEC], [0xDDu8, 0xD8, 0xCA]);
        for y in 0..hh {
            for x in 0..w {
                // distância com sinal até o retângulo arredondado
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                let qx = (px - w as f32 / 2.0).abs() - (w as f32 / 2.0 - raio);
                let qy = (py - hh as f32 / 2.0).abs() - (hh as f32 / 2.0 - raio);
                let fora = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt() + qx.max(qy).min(0.0) - raio;
                let cob = (0.5 - fora).clamp(0.0, 1.0);
                let na_borda = (fora + 1.0).clamp(0.0, 1.0); // 1 px de borda
                let c: [f32; 3] = std::array::from_fn(|k| bg[k] as f32 * (1.0 - na_borda) + borda[k] as f32 * na_borda);
                let a = cob * opac;
                let i = ((y * w + x) * 4) as usize;
                *bits.add(i) = (c[2] * a) as u8;
                *bits.add(i + 1) = (c[1] * a) as u8;
                *bits.add(i + 2) = (c[0] * a) as u8;
                *bits.add(i + 3) = (a * 255.0) as u8;
            }
        }
        let tela = GetDC(std::ptr::null_mut());
        let dc = CreateCompatibleDC(tela);
        let antigo = SelectObject(dc, bmp);
        let pos = POINT { x: r.left, y: r.top };
        let tam = SIZE { cx: w, cy: hh };
        let origem = POINT { x: 0, y: 0 };
        let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8 };
        UpdateLayeredWindow(h, tela, &pos, &tam, dc, &origem, 0, &blend, ULW_ALPHA);
        SelectObject(dc, antigo);
        DeleteObject(bmp);
        DeleteDC(dc);
        ReleaseDC(std::ptr::null_mut(), tela);
        if IsWindowVisible(h) == 0 {
            SetWindowPos(h, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW);
        }
    }
}

/// Recolhe a janela do chat (no retângulo r) até a bolha.
fn recolher_de(r: RECT) {
    let b = rect_bolha();
    iniciar_anim(r, b, false);
}

fn alternar_ui() {
    let h = janela_ui();
    unsafe {
        if !h.is_null() && GetForegroundWindow() == h {
            let mut r: RECT = std::mem::zeroed();
            GetWindowRect(h, &mut r);
            ShowWindow(h, SW_HIDE); // só esconde: continua carregado para a próxima vez
            recolher_de(r);
        } else {
            abrir_ui(false);
        }
    }
}

fn copiar(dest: &mut [u16], s: &str) {
    let w: Vec<u16> = s.encode_utf16().take(dest.len() - 1).collect();
    dest[..w.len()].copy_from_slice(&w);
    dest[w.len()] = 0;
}

fn notificar(titulo: &str, texto: &str) {
    unsafe {
        let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
        nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        nid.hWnd = JANELA.load(Ordering::Relaxed) as HWND;
        nid.uID = 1;
        nid.uFlags = NIF_INFO;
        copiar(&mut nid.szInfoTitle, titulo);
        copiar(&mut nid.szInfo, texto);
        nid.dwInfoFlags = NIIF_USER | NIIF_LARGE_ICON;
        Shell_NotifyIconW(NIM_MODIFY, &nid);
    }
}

/// Alerta novo (já gravado na conversa): abre a janela, ou notifica se estiver jogando.
fn avisar(texto: &str) {
    if jogando() {
        est().aviso = true;
        pintar_bolha();
        notificar("Claudinho", texto);
    } else {
        abrir_ui(true);
    }
}

// ---------- ícones ----------
unsafe fn dib_32(n: i32) -> (HBITMAP, *mut u8) {
    unsafe { dib(n, n) }
}
unsafe fn dib(w: i32, hh: i32) -> (HBITMAP, *mut u8) {
    unsafe {
        let mut bmi: BITMAPINFO = std::mem::zeroed();
        bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bmi.bmiHeader.biWidth = w;
        bmi.bmiHeader.biHeight = -hh; // de cima para baixo
        bmi.bmiHeader.biPlanes = 1;
        bmi.bmiHeader.biBitCount = 32;
        bmi.bmiHeader.biCompression = BI_RGB;
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let tela = GetDC(std::ptr::null_mut());
        let bmp = CreateDIBSection(tela, &bmi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
        ReleaseDC(std::ptr::null_mut(), tela);
        (bmp, bits as *mut u8)
    }
}

fn criar_hicon(n: u32) -> HICON {
    unsafe {
        let rgba = mascote_rgba(n);
        let (cor, bits) = dib_32(n as i32);
        for i in 0..(n * n) as usize {
            *bits.add(i * 4) = rgba[i * 4 + 2];
            *bits.add(i * 4 + 1) = rgba[i * 4 + 1];
            *bits.add(i * 4 + 2) = rgba[i * 4];
            *bits.add(i * 4 + 3) = rgba[i * 4 + 3];
        }
        let mascara = CreateBitmap(n as i32, n as i32, 1, 1, std::ptr::null());
        let ii = ICONINFO { fIcon: 1, xHotspot: 0, yHotspot: 0, hbmMask: mascara, hbmColor: cor };
        let h = CreateIconIndirect(&ii);
        DeleteObject(mascara);
        DeleteObject(cor);
        h
    }
}

// ---------- bolha flutuante ----------
fn arquivo_posicao() -> std::path::PathBuf {
    caminho_ico().with_file_name("bolha.txt")
}
fn area() -> RECT {
    let mut r = RECT { left: 0, top: 0, right: 1920, bottom: 1080 };
    unsafe { SystemParametersInfoW(SPI_GETWORKAREA, 0, &mut r as *mut _ as *mut _, 0) };
    r
}
fn posicao_inicial() -> (i32, i32) {
    if let Ok(s) = std::fs::read_to_string(arquivo_posicao()) {
        if let Some((x, y)) = s.trim().split_once(',') {
            if let (Ok(x), Ok(y)) = (x.parse(), y.parse()) {
                return (x, y);
            }
        }
    }
    let a = area();
    (a.right - BOLHA - 8, a.bottom - BOLHA - 140)
}

/// Desenha a bolha (sombra suave + mascote + pontinho de aviso) e atualiza a janela em camadas.
fn pintar_bolha() {
    unsafe {
        let h = BOLHA_H.load(Ordering::Relaxed) as HWND;
        let e = est();
        let n = BOLHA;
        let (bmp, bits) = dib_32(n);
        let m = mascote_rgba(MASCOTE);
        let off = (n - MASCOTE as i32) / 2;
        let c = n as f32 / 2.0;
        for y in 0..n {
            for x in 0..n {
                let i = ((y * n + x) * 4) as usize;
                // sombra
                let (dx, dy) = (x as f32 + 0.5 - c, y as f32 + 0.5 - c - 2.0);
                let r = (dx * dx + dy * dy).sqrt();
                let mut a = (0.28 * (1.0 - (r - MASCOTE as f32 * 0.42) / 7.0)).clamp(0.0, 0.28);
                let (mut cr, mut cg, mut cb) = (0.0f32, 0.0f32, 0.0f32);
                let (mx, my) = (x - off, y - off);
                if mx >= 0 && my >= 0 && mx < MASCOTE as i32 && my < MASCOTE as i32 {
                    let j = ((my as u32 * MASCOTE + mx as u32) * 4) as usize;
                    let ma = m[j + 3] as f32 / 255.0;
                    cr = m[j] as f32 * ma;
                    cg = m[j + 1] as f32 * ma;
                    cb = m[j + 2] as f32 * ma;
                    a = ma + a * (1.0 - ma);
                    // sombra marrom quente por baixo
                    let s = a - ma;
                    cr += 60.0 * s;
                    cg += 42.0 * s;
                    cb += 32.0 * s;
                } else {
                    cr = 60.0 * a;
                    cg = 42.0 * a;
                    cb = 32.0 * a;
                }
                // pontinho de aviso (creme com borda terracota) no canto superior direito
                if e.aviso {
                    let (px, py) = (x as f32 + 0.5 - (n as f32 - 13.0), y as f32 + 0.5 - 13.0);
                    let d = (px * px + py * py).sqrt();
                    if d < 7.0 {
                        let (rr, gg, bb) = if d < 5.0 { (255.0, 248.0, 238.0) } else { (192.0, 96.0, 63.0) };
                        cr = rr;
                        cg = gg;
                        cb = bb;
                        a = 1.0;
                    }
                }
                // alfa mínimo dentro do círculo: a bolha inteira recebe clique (alfa 0 deixa o clique passar)
                let (qx, qy) = (x as f32 + 0.5 - c, y as f32 + 0.5 - c);
                if (qx * qx + qy * qy).sqrt() < c && a < 0.02 {
                    a = 0.02;
                }
                *bits.add(i) = cb.min(255.0) as u8;
                *bits.add(i + 1) = cg.min(255.0) as u8;
                *bits.add(i + 2) = cr.min(255.0) as u8;
                *bits.add(i + 3) = (a * 255.0) as u8;
            }
        }
        let antigo = SelectObject(e.dc, bmp);
        let tam = SIZE { cx: n, cy: n };
        let origem = POINT { x: 0, y: 0 };
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: if e.hover || e.arrastando { 255 } else { 128 },
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        let tela = GetDC(std::ptr::null_mut());
        UpdateLayeredWindow(h, tela, std::ptr::null(), &tam, e.dc, &origem, 0, &blend, ULW_ALPHA);
        ReleaseDC(std::ptr::null_mut(), tela);
        SelectObject(e.dc, antigo);
        DeleteObject(bmp);
    }
}

fn mostrar_bolha(sim: bool) {
    let e = est();
    if e.visivel == sim {
        return;
    }
    e.visivel = sim;
    unsafe {
        let h = BOLHA_H.load(Ordering::Relaxed) as HWND;
        // o mouse da bolha vem de um gancho de baixo nível, ligado só enquanto ela aparece
        let g = GANCHO_MOUSE.load(Ordering::Relaxed);
        if sim && g == 0 {
            let novo = SetWindowsHookExW(WH_MOUSE_LL, Some(gancho_mouse), GetModuleHandleW(std::ptr::null()), 0);
            GANCHO_MOUSE.store(novo as isize, Ordering::Relaxed);
        } else if !sim && g != 0 {
            UnhookWindowsHookEx(g as HHOOK);
            GANCHO_MOUSE.store(0, Ordering::Relaxed);
            e.hover = false;
            e.arrastando = false;
        }
        if sim {
            pintar_bolha();
            SetWindowPos(h, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW);
        } else {
            ShowWindow(h, SW_HIDE);
        }
    }
}

fn grudar_na_borda(h: HWND) {
    unsafe {
        let mut r: RECT = std::mem::zeroed();
        GetWindowRect(h, &mut r);
        let a = area();
        // fica onde foi solta; só volta para dentro se saiu da tela
        let x = r.left.clamp(a.left, a.right - BOLHA);
        let y = r.top.clamp(a.top, a.bottom - BOLHA);
        // pequena animação até a borda
        let passos = 8;
        for i in 1..=passos {
            let t = i as f32 / passos as f32;
            let t = 1.0 - (1.0 - t) * (1.0 - t);
            let xi = r.left + ((x - r.left) as f32 * t) as i32;
            let yi = r.top + ((y - r.top) as f32 * t) as i32;
            SetWindowPos(h, std::ptr::null_mut(), xi, yi, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
            std::thread::sleep(std::time::Duration::from_millis(12));
        }
        let _ = std::fs::write(arquivo_posicao(), format!("{x},{y}"));
    }
}

unsafe extern "system" fn proc_bolha(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        #[allow(static_mut_refs)]
        if BOLHA_EST.is_none() {
            return DefWindowProcW(h, msg, wp, lp); // mensagens da criação da janela
        }
        match msg {
            WM_BOLHA_SOLTOU => {
                if wp != 0 {
                    grudar_na_borda(h);
                } else {
                    abrir_ui(false);
                }
                pintar_bolha();
                0
            }
            WM_BOLHA_MENU => {
                menu(JANELA.load(Ordering::Relaxed) as HWND);
                0
            }
            WM_MOUSEACTIVATE => MA_NOACTIVATE as LRESULT,
            _ => DefWindowProcW(h, msg, wp, lp),
        }
    }
}

/// Gancho de mouse de baixo nível: hover, clique, arrasto e menu da bolha.
/// Só fica ligado enquanto a bolha está visível (nunca durante jogo em tela cheia).
unsafe extern "system" fn gancho_mouse(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        #[allow(static_mut_refs)]
        if code < 0 || BOLHA_EST.is_none() {
            return CallNextHookEx(std::ptr::null_mut(), code, wp, lp);
        }
        let e = est();
        let h = BOLHA_H.load(Ordering::Relaxed) as HWND;
        let m = &*(lp as *const MSLLHOOKSTRUCT);
        let p = m.pt;
        let mut r: RECT = std::mem::zeroed();
        GetWindowRect(h, &mut r);
        let dentro = e.visivel && p.x >= r.left && p.x < r.right && p.y >= r.top && p.y < r.bottom;
        match wp as u32 {
            WM_MOUSEMOVE => {
                if e.arrastando {
                    let (dx, dy) = (p.x - e.inicio_cursor.x, p.y - e.inicio_cursor.y);
                    if dx.abs() + dy.abs() > 4 {
                        e.moveu = true;
                    }
                    if e.moveu {
                        SetWindowPos(h, std::ptr::null_mut(), e.inicio_janela.x + dx, e.inicio_janela.y + dy, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
                    }
                } else if dentro != e.hover {
                    e.hover = dentro;
                    pintar_bolha();
                }
            }
            WM_LBUTTONDOWN if dentro => {
                e.arrastando = true;
                e.moveu = false;
                e.inicio_cursor = p;
                e.inicio_janela = POINT { x: r.left, y: r.top };
                pintar_bolha();
                return 1; // não deixa o clique cair na janela de baixo
            }
            WM_LBUTTONUP if e.arrastando => {
                e.arrastando = false;
                PostMessageW(h, WM_BOLHA_SOLTOU, e.moveu as usize, 0);
                return 1;
            }
            WM_RBUTTONDOWN if dentro => return 1,
            WM_RBUTTONUP if dentro => {
                PostMessageW(h, WM_BOLHA_MENU, 0, 0);
                return 1;
            }
            _ => {}
        }
        CallNextHookEx(std::ptr::null_mut(), code, wp, lp)
    }
}

fn atualizar_bolha() {
    if animando() {
        return;
    }
    let ui = janela_ui();
    // chat escondido (fechando) conta como fechado
    if !ui.is_null() && unsafe { IsWindowVisible(ui) } == 0 {
        mostrar_bolha(!jogando());
        return;
    }
    let deve = janela_ui().is_null() && !jogando();
    mostrar_bolha(deve);
}

// ---------- verificações ----------
fn tique() {
    for l in lembretes_vencidos() {
        let t = format!("⏰ Lembrete: {l}");
        gravar("alerta", Modelo::Haiku, &t);
        avisar(&t);
    }
    // chat escondido: devolve a RAM dele ao Windows (ele continua pronto para abrir na hora)
    if chat_escondido() {
        aparar_ui();
    } else if janela_ui().is_null() && !jogando() && !animando() {
        pre_carregar_ui(); // o chat caiu ou foi fechado: recarrega escondido
    }
    // memória longa: dobra as mensagens antigas no resumo, em segundo plano, fora de jogo
    if !jogando() && !COMPACTANDO.load(Ordering::Relaxed) && precisa_compactar() {
        COMPACTANDO.store(true, Ordering::Relaxed);
        std::thread::spawn(|| {
            if let Err(e) = compactar() {
                let _ = std::fs::write(pasta_dia().join("memoria-longa.erro.txt"), format!("{} {}: {e}", data_iso(&agora()), hhmm(&agora())));
            }
            COMPACTANDO.store(false, Ordering::Relaxed);
        });
    }
    let t = agora();
    let chave = format!("{} {:02}", data_iso(&t), t.hora);
    let mut ult = ULTIMA_HORA.lock().unwrap();
    if *ult == chave || !(9..=22).contains(&t.hora) || t.min >= 10 {
        return;
    }
    *ult = chave;
    drop(ult);
    if jogando() || OCUPADO.load(Ordering::Relaxed) || chat_visivel() {
        return;
    }
    let (conv, _) = carregar_ultimas(200);
    if let Some(m) = conv.iter().rev().find(|m| m.papel == "eu") {
        let (h, mi) = m.hora.split_once(':').unwrap_or(("0", "0"));
        let min_msg = h.parse::<i32>().unwrap_or(0) * 60 + mi.parse::<i32>().unwrap_or(0);
        if m.data == data_iso(&t) && (t.hora as i32 * 60 + t.min as i32) - min_msg < 20 {
            return;
        }
    }
    OCUPADO.store(true, Ordering::Relaxed);
    let hora = hhmm(&t);
    let lider = lider_atual(&conv);
    std::thread::spawn(move || {
        trabalhar(Pedido::Verificacao(hora), lider, |r| {
            if let Resultado::Resposta { modelo, texto, .. } = r {
                if !texto.is_empty() {
                    gravar("alerta", modelo, &texto);
                    ALERTAS.lock().unwrap().push(texto);
                }
            }
        });
        OCUPADO.store(false, Ordering::Relaxed);
        unsafe { PostMessageW(JANELA.load(Ordering::Relaxed) as HWND, WM_RESULTADO, 0, 0) };
    });
}


fn menu(h: HWND) {
    unsafe {
        let m = CreatePopupMenu();
        AppendMenuW(m, MF_STRING, ID_ABRIR, wide("Abrir conversa").as_ptr());
        AppendMenuW(m, MF_STRING, ID_HISTORICO, wide("Histórico completo (pasta)").as_ptr());
        AppendMenuW(m, MF_STRING, ID_MEMORIA, wide("O que o Claude lembra (memória longa)").as_ptr());
        AppendMenuW(m, MF_STRING, ID_SAIR, wide("Sair do Claudinho").as_ptr());
        let mut p = POINT { x: 0, y: 0 };
        GetCursorPos(&mut p);
        SetForegroundWindow(h);
        let cmd = TrackPopupMenu(m, TPM_RETURNCMD | TPM_RIGHTBUTTON, p.x, p.y, 0, h, std::ptr::null());
        DestroyMenu(m);
        match cmd as usize {
            ID_ABRIR => abrir_ui(false),
            ID_HISTORICO => {
                let _ = std::fs::create_dir_all(pasta_conversa());
                let _ = std::process::Command::new("explorer.exe").arg(pasta_conversa()).spawn();
            }
            ID_MEMORIA => {
                let p = arquivo_resumo();
                if !p.exists() {
                    let _ = std::fs::write(&p, "(A memória longa ainda está vazia: ela começa a ser escrita depois de umas 40 mensagens.)\n");
                }
                let _ = std::process::Command::new("notepad.exe").arg(p).spawn();
            }
            ID_SAIR => {
                let u = janela_ui();
                if !u.is_null() {
                    PostMessageW(u, WM_CLOSE, 0, 0);
                }
                DestroyWindow(h);
            }
            _ => {}
        }
    }
}

unsafe extern "system" fn proc_janela(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_TIMER => {
                if wp == TIMER_TIQUE {
                    tique();
                } else if wp == TIMER_ANIM {
                    quadro_anim();
                } else {
                    atualizar_bolha();
                }
                0
            }
            WM_ATALHO => {
                alternar_ui();
                0
            }
            WM_ABRIR => {
                abrir_ui(false);
                0
            }
            WM_UI_FECHOU => {
                // o chat avisa seu retângulo ao fechar: x,y em wp; largura,altura em lp
                let (x, y) = ((wp & 0xFFFF) as u16 as i16 as i32, ((wp >> 16) & 0xFFFF) as u16 as i16 as i32);
                let (w, hh) = ((lp as usize & 0xFFFF) as i32, ((lp as usize >> 16) & 0xFFFF) as i32);
                recolher_de(RECT { left: x, top: y, right: x + w, bottom: y + hh });
                0
            }
            WM_RESULTADO => {
                let v: Vec<String> = ALERTAS.lock().unwrap().drain(..).collect();
                for t in v {
                    avisar(&t);
                }
                0
            }
            WM_TRAY => {
                let ev = lp as u32 & 0xFFFF;
                if ev == WM_LBUTTONUP {
                    alternar_ui();
                } else if ev == WM_RBUTTONUP {
                    menu(h);
                }
                0
            }
            WM_DESTROY => {
                let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
                nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
                nid.hWnd = h;
                nid.uID = 1;
                Shell_NotifyIconW(NIM_DELETE, &nid);
                PostQuitMessage(0);
                0
            }
            _ => DefWindowProcW(h, msg, wp, lp),
        }
    }
}

fn main() {
    let gerar = std::env::args().any(|a| a == "--gerar-ico");
    if gerar || !caminho_ico().exists() {
        let _ = salvar_ico(&caminho_ico(), &[16, 24, 32, 48, 64, 128, 256]);
    }
    if gerar {
        return;
    }
    unsafe {
        CreateMutexW(std::ptr::null(), 1, wide("Local\\ClaudinhoVigia").as_ptr());
        if GetLastError() == ERROR_ALREADY_EXISTS {
            // já está rodando: pede ao vigia que abra o chat
            let v = FindWindowW(wide("ClaudinhoVigia").as_ptr(), std::ptr::null());
            if !v.is_null() {
                PostMessageW(v, WM_ABRIR, 0, 0);
            }
            return;
        }
        let inst = GetModuleHandleW(std::ptr::null());
        let icone_grande = criar_hicon(32);
        let icone_peq = criar_hicon(16);

        // janela invisível (mensagens, bandeja, timers)
        let classe = wide("ClaudinhoVigia");
        let mut wc: WNDCLASSW = std::mem::zeroed();
        wc.lpfnWndProc = Some(proc_janela);
        wc.hInstance = inst;
        wc.hIcon = icone_grande;
        wc.lpszClassName = classe.as_ptr();
        RegisterClassW(&wc);
        let h = CreateWindowExW(0, classe.as_ptr(), wide("Claudinho (vigia)").as_ptr(), 0, 0, 0, 0, 0, std::ptr::null_mut(), std::ptr::null_mut(), inst, std::ptr::null());
        JANELA.store(h as isize, Ordering::Relaxed);

        // bolha flutuante
        let classe_b = wide("ClaudinhoBolha");
        let mut wb: WNDCLASSW = std::mem::zeroed();
        wb.lpfnWndProc = Some(proc_bolha);
        wb.hInstance = inst;
        wb.hCursor = LoadCursorW(std::ptr::null_mut(), IDC_HAND);
        wb.lpszClassName = classe_b.as_ptr();
        RegisterClassW(&wb);
        let (bx, by) = posicao_inicial();
        let hb = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            classe_b.as_ptr(),
            wide("Claudinho (bolha)").as_ptr(),
            WS_POPUP,
            bx,
            by,
            BOLHA,
            BOLHA,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            inst,
            std::ptr::null(),
        );
        BOLHA_H.store(hb as isize, Ordering::Relaxed);

        // janela fantasma da animação (não recebe clique)
        let classe_a = wide("ClaudinhoAnim");
        let mut wa: WNDCLASSW = std::mem::zeroed();
        wa.lpfnWndProc = Some(DefWindowProcW);
        wa.hInstance = inst;
        wa.lpszClassName = classe_a.as_ptr();
        RegisterClassW(&wa);
        let ha = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT,
            classe_a.as_ptr(),
            wide("Claudinho (anim)").as_ptr(),
            WS_POPUP,
            0,
            0,
            1,
            1,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            inst,
            std::ptr::null(),
        );
        ANIM_H.store(ha as isize, Ordering::Relaxed);
        let tela = GetDC(std::ptr::null_mut());
        BOLHA_EST = Some(EstadoBolha {
            dc: CreateCompatibleDC(tela),
            visivel: false,
            hover: false,
            arrastando: false,
            moveu: false,
            inicio_cursor: POINT { x: 0, y: 0 },
            inicio_janela: POINT { x: 0, y: 0 },
            aviso: false,
        });
        ReleaseDC(std::ptr::null_mut(), tela);

        // bandeja
        let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
        nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        nid.hWnd = h;
        nid.uID = 1;
        nid.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
        nid.uCallbackMessage = WM_TRAY;
        nid.hIcon = icone_peq;
        nid.hBalloonIcon = icone_grande;
        copiar(&mut nid.szTip, "Claudinho");
        Shell_NotifyIconW(NIM_ADD, &nid);

        SetTimer(h, TIMER_TIQUE, 20_000, None);
        SetTimer(h, TIMER_BOLHA, 1_000, None);
        migrar_conversas_antigas();
        pre_carregar_ui();
        atualizar_bolha();
        tique();

        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
