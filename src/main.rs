#![windows_subsystem = "windows"]
//! claude-popup — janela pequena, sempre no topo, sem barra do Windows.
//! O Claude abre com uma mensagem; o usuário responde; a resposta sai no stdout.
//!
//! Uso:
//!   claude-popup --titulo "Lembrete" --msg "Texto\nOutra linha" [--opcoes "Sim|Não"]
//!                [--info] [--fechar 8] [--saida arquivo.txt] [--dica "Responda aqui"]
//! Saída: resposta no stdout (UTF-8). Código 0 = respondeu, 1 = fechou sem responder.

use eframe::egui::{
    self, Align, Color32, CornerRadius, FontId, Key, Layout, Margin, Modifiers, Pos2, Rect,
    RichText, Sense, Stroke, Vec2, ViewportCommand,
};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

// Paleta do Claudinho: creme, marrom quente e verde-azulado. Sem preto e sem vermelho puro.
const BG: Color32 = Color32::from_rgb(0xF5, 0xF3, 0xEC);
const BORDA: Color32 = Color32::from_rgb(0xDD, 0xD8, 0xCA);
const TEXTO: Color32 = Color32::from_rgb(0x3D, 0x39, 0x29);
const SUAVE: Color32 = Color32::from_rgb(0x83, 0x7D, 0x6B);
const ACENTO: Color32 = Color32::from_rgb(0x1E, 0x9C, 0x8C);
const ACENTO_HOVER: Color32 = Color32::from_rgb(0x17, 0x84, 0x77);
const CAMPO: Color32 = Color32::from_rgb(0xFF, 0xFE, 0xFB);
const CHIP: Color32 = Color32::from_rgb(0xEC, 0xE8, 0xDD);

const LARGURA: f32 = 440.0;
const MARGEM_TELA: f32 = 18.0;

#[derive(Default, Clone)]
struct Args {
    titulo: String,
    msg: String,
    opcoes: Vec<String>,
    info: bool,
    fechar: Option<f64>,
    saida: Option<String>,
    dica: String,
}

fn ler_args() -> Args {
    let mut a = Args {
        titulo: "Claudinho".into(),
        dica: "Responda aqui…  (Enter envia · Shift+Enter quebra linha)".into(),
        ..Default::default()
    };
    let mut it = std::env::args().skip(1);
    while let Some(k) = it.next() {
        match k.as_str() {
            "--titulo" => a.titulo = it.next().unwrap_or_default(),
            "--msg" => a.msg = it.next().unwrap_or_default().replace("\\n", "\n"),
            "--opcoes" => {
                a.opcoes = it
                    .next()
                    .unwrap_or_default()
                    .split('|')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            }
            "--info" => a.info = true,
            "--fechar" => a.fechar = it.next().and_then(|s| s.parse().ok()),
            "--saida" => a.saida = it.next(),
            "--dica" => a.dica = it.next().unwrap_or_default(),
            _ => {}
        }
    }
    a
}

// Área de trabalho (tela menos a barra de tarefas), para abrir no canto inferior direito.
#[repr(C)]
#[derive(Default)]
struct RectW {
    l: i32,
    t: i32,
    r: i32,
    b: i32,
}
#[cfg(windows)]
#[link(name = "user32")]
unsafe extern "system" {
    fn SystemParametersInfoW(acao: u32, p1: u32, p2: *mut core::ffi::c_void, ini: u32) -> i32;
    fn FindWindowW(classe: *const u16, titulo: *const u16) -> isize;
}
#[cfg(windows)]
#[link(name = "dwmapi")]
unsafe extern "system" {
    fn DwmSetWindowAttribute(hwnd: isize, attr: u32, valor: *const core::ffi::c_void, tam: u32) -> i32;
}

/// Windows 11: cantos arredondados nativos, sem precisar de janela transparente.
fn arredondar(titulo: &str) {
    #[cfg(windows)]
    unsafe {
        let w: Vec<u16> = titulo.encode_utf16().chain(Some(0)).collect();
        let h = FindWindowW(std::ptr::null(), w.as_ptr());
        if h != 0 {
            let pref: u32 = 2; // DWMWCP_ROUND
            DwmSetWindowAttribute(h, 33, &pref as *const u32 as *const _, 4);
        }
    }
}

static T0: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

fn log(msg: &str) {
    let msg = format!("[{:>5} ms] {msg}", T0.get_or_init(Instant::now).elapsed().as_millis());
    if std::env::var_os("CLAUDE_POPUP_LOG").is_some() {
        use std::io::Write;
        let p = std::env::temp_dir().join("claude-popup").join("log.txt");
        let _ = std::fs::create_dir_all(p.parent().unwrap());
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(p) {
            let _ = writeln!(f, "{msg}");
        }
    }
}

fn arquivo_padrao() -> std::path::PathBuf {
    std::env::temp_dir().join("claude-popup").join("resposta.txt")
}
fn area_trabalho() -> Option<Rect> {
    #[cfg(windows)]
    unsafe {
        let mut r = RectW::default();
        if SystemParametersInfoW(0x0030, 0, &mut r as *mut _ as *mut _, 0) != 0 {
            return Some(Rect::from_min_max(
                Pos2::new(r.l as f32, r.t as f32),
                Pos2::new(r.r as f32, r.b as f32),
            ));
        }
    }
    None
}

struct App {
    a: Args,
    resposta: String,
    resultado: Rc<RefCell<Option<String>>>,
    inicio: Instant,
    altura: f32,
    focou: bool,
    quadros: u32,
    ajustes: u32,
    visivel: bool,
    titulo_janela: String,
}

impl App {
    fn enviar(&mut self, ctx: &egui::Context, texto: String) {
        let t = texto.trim().to_string();
        if t.is_empty() && !self.a.info {
            return;
        }
        let t = if t.is_empty() { "ok".into() } else { t };
        let p = self.a.saida.clone().map(Into::into).unwrap_or_else(arquivo_padrao);
        let _ = std::fs::create_dir_all(std::path::Path::new(&p).parent().unwrap_or(std::path::Path::new(".")));
        let _ = std::fs::write(&p, &t);
        log(&format!("enviado: {t}"));
        *self.resultado.borrow_mut() = Some(t);
        ctx.send_viewport_cmd(ViewportCommand::Close);
    }

    fn posicionar(&self, ctx: &egui::Context) {
        if let Some(w) = area_trabalho() {
            let ppp = ctx.pixels_per_point();
            // A área vem em pixels físicos se o processo já é DPI-aware; converte para pontos.
            let (dir, baixo) = (w.max.x / ppp, w.max.y / ppp);
            ctx.send_viewport_cmd(ViewportCommand::OuterPosition(Pos2::new(
                dir - LARGURA - MARGEM_TELA,
                baixo - self.altura - MARGEM_TELA,
            )));
        }
    }
}

fn logo(p: &egui::Painter, c: Pos2, r: f32) {
    // o mascote: balãozinho de conversa sorrindo
    let escuro = Color32::from_rgb(0x18, 0x3B, 0x36);
    p.add(egui::Shape::convex_polygon(
        vec![c + Vec2::new(-0.62, 0.50) * r, c + Vec2::new(-0.92, 0.93) * r, c + Vec2::new(-0.22, 0.80) * r],
        ACENTO,
        Stroke::NONE,
    ));
    p.circle_filled(c, r * 0.86, ACENTO);
    for sx in [-1.0f32, 1.0] {
        p.circle_filled(c + Vec2::new(sx * 0.27, -0.10) * r, r * 0.11, escuro);
    }
    let pts: Vec<Pos2> = (0..=10)
        .map(|i| {
            let a = std::f32::consts::PI * (0.22 + 0.56 * i as f32 / 10.0);
            c + Vec2::new(a.cos() * 0.36, a.sin() * 0.36 - 0.02) * r
        })
        .collect();
    p.add(egui::Shape::line(pts, Stroke::new((r * 0.09).max(1.2), escuro)));
}

impl eframe::App for App {
    fn clear_color(&self, _: &egui::Visuals) -> [f32; 4] {
        egui::Rgba::from(BG).to_array()
    }

    fn ui(&mut self, raiz: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx_dono = raiz.ctx().clone();
        let ctx = &ctx_dono;
        if let Some(s) = self.a.fechar {
            let resta = s - self.inicio.elapsed().as_secs_f64();
            if resta <= 0.0 {
                ctx.send_viewport_cmd(ViewportCommand::Close);
            } else {
                ctx.request_repaint_after(std::time::Duration::from_millis(250));
            }
        }
        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }
        let enter = ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter));

        let painel = egui::Frame::NONE
            .fill(BG)
            .stroke(Stroke::new(1.0, BORDA))
            .inner_margin(Margin::symmetric(18, 14));

        let mut acao: Option<String> = None;
        let mut fechar = false;

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(raiz, |ui| {
                let resp = painel.show(ui, |ui| {
                    ui.set_width(LARGURA - 38.0);
                    ui.spacing_mut().item_spacing = Vec2::new(8.0, 8.0);

                    // Cabeçalho arrastável.
                    let (cab, cab_resp) =
                        ui.allocate_exact_size(Vec2::new(ui.available_width(), 30.0), Sense::click_and_drag());
                    if cab_resp.drag_started() {
                        ctx.send_viewport_cmd(ViewportCommand::StartDrag);
                    }
                    let p = ui.painter();
                    logo(p, Pos2::new(cab.min.x + 12.0, cab.center().y), 11.0);
                    p.text(
                        Pos2::new(cab.min.x + 32.0, cab.center().y - 7.0),
                        egui::Align2::LEFT_CENTER,
                        &self.a.titulo,
                        forte(16.0),
                        TEXTO,
                    );
                    p.text(
                        Pos2::new(cab.min.x + 32.0, cab.center().y + 9.0),
                        egui::Align2::LEFT_CENTER,
                        "Claude Code",
                        FontId::proportional(11.0),
                        SUAVE,
                    );
                    let x = Rect::from_center_size(Pos2::new(cab.max.x - 10.0, cab.center().y), Vec2::splat(22.0));
                    let xr = ui.interact(x, ui.id().with("fechar"), Sense::click());
                    if xr.hovered() {
                        p.circle_filled(x.center(), 11.0, CHIP);
                    }
                    p.text(x.center(), egui::Align2::CENTER_CENTER, "×", FontId::proportional(18.0), SUAVE);
                    if xr.clicked() {
                        fechar = true;
                    }

                    ui.add_space(2.0);
                    if !self.a.msg.is_empty() {
                        ui.label(RichText::new(&self.a.msg).size(14.0).color(TEXTO));
                    }

                    if !self.a.opcoes.is_empty() {
                        ui.add_space(2.0);
                        ui.horizontal_wrapped(|ui| {
                            for o in self.a.opcoes.clone() {
                                let b = egui::Button::new(RichText::new(&o).size(13.0).color(TEXTO))
                                    .fill(CHIP)
                                    .stroke(Stroke::new(1.0, BORDA))
                                    .corner_radius(CornerRadius::same(16));
                                if ui.add(b).clicked() {
                                    log("opcao");
                                    acao = Some(o);
                                }
                            }
                        });
                    }

                    if !self.a.info {
                        let te = egui::TextEdit::multiline(&mut self.resposta)
                            .hint_text(RichText::new(&self.a.dica).color(SUAVE))
                            .desired_rows(3)
                            .desired_width(f32::INFINITY)
                            .text_color(TEXTO)
                            .background_color(CAMPO)
                            .margin(Margin::same(8));
                        let r = ui.add(te);
                        if !self.focou {
                            r.request_focus();
                            self.focou = true;
                        }
                    }

                    let larg = ui.available_width();
                    ui.allocate_ui_with_layout(Vec2::new(larg, 32.0), Layout::right_to_left(Align::Center), |ui| {
                        let rotulo = if self.a.info { "Ok" } else { "Enviar" };
                        let b = egui::Button::new(RichText::new(rotulo).font(forte(14.0)).color(Color32::WHITE))
                            .fill(ACENTO)
                            .corner_radius(CornerRadius::same(8))
                            .min_size(Vec2::new(84.0, 30.0));
                        let r = ui.add(b);
                        if r.hovered() {
                            ui.painter().rect_filled(r.rect, CornerRadius::same(8), ACENTO_HOVER);
                            ui.painter().text(
                                r.rect.center(),
                                egui::Align2::CENTER_CENTER,
                                rotulo,
                                forte(14.0),
                                Color32::WHITE,
                            );
                        }
                        if r.clicked() {
                            log("botao enviar");
                            acao = Some(self.resposta.clone());
                        }
                        if let Some(s) = self.a.fechar {
                            let resta = (s - self.inicio.elapsed().as_secs_f64()).max(0.0).ceil();
                            ui.label(RichText::new(format!("fecha em {resta:.0}s")).size(11.0).color(SUAVE));
                        }
                    });
                    ui.min_rect().height()
                });

                // Ajusta a altura da janela ao conteúdo e mantém no canto.
                let h = (resp.inner + 28.0 + 2.0).ceil();
                self.quadros += 1;
                if (h - self.altura).abs() > 1.0 && self.ajustes < 6 {
                    self.altura = h;
                    self.ajustes += 1;
                    ctx.send_viewport_cmd(ViewportCommand::InnerSize(Vec2::new(LARGURA, h)));
                    self.posicionar(ctx);
                    log(&format!("quadro {} ajuste {} altura {h} ppp {}", self.quadros, self.ajustes, ctx.pixels_per_point()));
                    ctx.request_repaint();
                } else if !self.visivel && self.quadros >= 2 {
                    self.visivel = true;
                    ctx.send_viewport_cmd(ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(ViewportCommand::Focus);
                    arredondar(&self.titulo_janela);
                    log(&format!("visivel no quadro {}", self.quadros));
                }
            });

        if enter && self.visivel {
            log("tecla enter");
            acao = Some(self.resposta.clone());
        }
        if fechar {
            ctx.send_viewport_cmd(ViewportCommand::Close);
        } else if let Some(t) = acao {
            self.enviar(ctx, t);
        }
    }
}

/// Segoe UI do Windows (mais nítida que a fonte fina padrão do egui em tela 100%).
/// As fontes padrão ficam como reserva para emoji e símbolos.
fn fontes_windows(ctx: &egui::Context) {
    use egui::{FontData, FontDefinitions, FontFamily};
    let mut f = FontDefinitions::default();
    let dir = std::path::Path::new(r"C:\Windows\Fonts");
    for (nome, arq) in [("segoe", "segoeui.ttf"), ("segoe-sb", "seguisb.ttf"), ("emoji", "seguiemj.ttf")] {
        if let Ok(b) = std::fs::read(dir.join(arq)) {
            f.font_data.insert(nome.into(), std::sync::Arc::new(FontData::from_owned(b)));
        }
    }
    let prop = f.families.entry(FontFamily::Proportional).or_default();
    let mut ordem = vec![];
    if f.font_data.contains_key("segoe") { ordem.push("segoe".to_string()); }
    if f.font_data.contains_key("emoji") { ordem.push("emoji".to_string()); }
    for (i, n) in ordem.into_iter().enumerate() { prop.insert(i, n); }
    if f.font_data.contains_key("segoe-sb") {
        f.families.insert(FontFamily::Name("forte".into()), vec!["segoe-sb".into(), "segoe".into()]);
    }
    ctx.set_fonts(f);
}

fn forte(tam: f32) -> FontId {
    FontId::new(tam, egui::FontFamily::Name("forte".into()))
}

fn main() {
    let a = ler_args();
    let resultado = Rc::new(RefCell::new(None));
    let titulo_janela = format!("Claude — {} #{}", a.titulo, std::process::id());
    log(&format!("inicio {titulo_janela}"));

    let vp = egui::ViewportBuilder::default()
        .with_title(titulo_janela.clone())
        .with_inner_size([LARGURA, 220.0])
        .with_decorations(false)
        .with_always_on_top()
        .with_resizable(false)
        .with_taskbar(true)
        .with_visible(false)
        .with_active(true);

    let opts = eframe::NativeOptions { viewport: vp, ..Default::default() };
    let r2 = resultado.clone();
    let _ = eframe::run_native(
        "claude-popup",
        opts,
        Box::new(move |cc| {
            let mut v = egui::Visuals::light();
            v.panel_fill = Color32::TRANSPARENT;
            v.window_fill = BG;
            v.selection.bg_fill = ACENTO.gamma_multiply(0.35);
            v.selection.stroke = Stroke::new(1.0, ACENTO);
            v.text_cursor.stroke = Stroke::new(2.0, ACENTO);
            v.widgets.inactive.bg_stroke = Stroke::new(1.0, BORDA);
            v.widgets.hovered.bg_stroke = Stroke::new(1.0, ACENTO);
            v.widgets.active.bg_stroke = Stroke::new(1.0, ACENTO);
            cc.egui_ctx.set_visuals(v);
            fontes_windows(&cc.egui_ctx);
            cc.egui_ctx.all_styles_mut(|s| {
                use egui::TextStyle::*;
                s.text_styles.insert(Body, FontId::proportional(14.0));
                s.text_styles.insert(Button, FontId::proportional(14.0));
                s.text_styles.insert(Small, FontId::proportional(11.0));
            });
            Ok(Box::new(App {
                a,
                resposta: String::new(),
                resultado: r2,
                inicio: Instant::now(),
                altura: 0.0,
                focou: false,
                quadros: 0,
                ajustes: 0,
                visivel: false,
                titulo_janela,
            }))
        }),
    );

    match resultado.borrow().as_ref() {
        Some(t) => {
            println!("{t}");
            std::process::exit(0)
        }
        None => std::process::exit(1),
    }
}
