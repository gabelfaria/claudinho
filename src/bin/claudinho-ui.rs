#![windows_subsystem = "windows"]
//! Janela de conversa do Claudinho. Fica pré-carregada e escondida (o vigia libera a RAM dela);
//! a conversa roda numa sessão contínua do Claude Code (sessao.rs), igual a uma conversa do app.

#[path = "../dia_comum.rs"]
mod comum;
#[path = "../sessao.rs"]
mod sessao;
use comum::*;
use sessao::{Evento, Sessao};

use eframe::egui::{
    self, Align, Color32, CornerRadius, FontId, Key, Layout, Margin, Modifiers, Pos2, Rect,
    RichText, Sense, Stroke, Vec2, ViewportCommand,
};
use std::path::Path;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant, SystemTime};

const BG: Color32 = Color32::from_rgb(0xF5, 0xF3, 0xEC);
const BORDA: Color32 = Color32::from_rgb(0xDD, 0xD8, 0xCA);
const TEXTO: Color32 = Color32::from_rgb(0x3D, 0x39, 0x29);
const SUAVE: Color32 = Color32::from_rgb(0x83, 0x7D, 0x6B);
const ACENTO: Color32 = Color32::from_rgb(0x1E, 0x9C, 0x8C);
const CAMPO: Color32 = Color32::from_rgb(0xFF, 0xFE, 0xFB);
const CHIP: Color32 = Color32::from_rgb(0xEC, 0xE8, 0xDD);
const BOLHA_EU: Color32 = Color32::from_rgb(0xD8, 0xF0, 0xEC);

const LARG: f32 = 400.0;
const ALT: f32 = 560.0;
const MARGEM: f32 = 18.0;

#[repr(C)]
#[derive(Default)]
struct RectW {
    l: i32,
    t: i32,
    r: i32,
    b: i32,
}
#[link(name = "user32")]
unsafe extern "system" {
    fn SystemParametersInfoW(acao: u32, p1: u32, p2: *mut core::ffi::c_void, ini: u32) -> i32;
    fn FindWindowW(classe: *const u16, titulo: *const u16) -> isize;
    fn ShowWindow(h: isize, cmd: i32) -> i32;
    fn SetForegroundWindow(h: isize) -> i32;
    fn GetDpiForSystem() -> u32;
    fn GetWindowRect(h: isize, r: *mut RectW) -> i32;
    fn PostMessageW(h: isize, msg: u32, wp: usize, lp: isize) -> i32;
}

/// Esconde a janela na hora (ela fica carregada para abrir rápido) e pede ao vigia a animação
/// de recolher até a bolha. Devolve false se o vigia não está rodando (aí a janela fecha de vez).
fn recolher() -> bool {
    unsafe {
        let h = FindWindowW(std::ptr::null(), wide(TITULO_UI).as_ptr());
        let vigia = FindWindowW(wide("ClaudinhoVigia").as_ptr(), std::ptr::null());
        if h == 0 || vigia == 0 {
            return false;
        }
        let mut r = RectW::default();
        GetWindowRect(h, &mut r);
        ShowWindow(h, 0); // SW_HIDE
        let wp = (r.l as u16 as usize) | ((r.t as u16 as usize) << 16);
        let lp = (((r.r - r.l) as u16 as usize) | (((r.b - r.t) as u16 as usize) << 16)) as isize;
        PostMessageW(vigia, 0x8000 + 12, wp, lp); // WM_APP + 12
        true
    }
}
#[link(name = "kernel32")]
unsafe extern "system" {
    fn CreateMutexW(a: *const core::ffi::c_void, dono: i32, nome: *const u16) -> isize;
    fn GetLastError() -> u32;
}
#[link(name = "dwmapi")]
unsafe extern "system" {
    fn DwmSetWindowAttribute(h: isize, attr: u32, v: *const core::ffi::c_void, tam: u32) -> i32;
}
fn area_trabalho() -> Option<Rect> {
    unsafe {
        let mut r = RectW::default();
        (SystemParametersInfoW(0x0030, 0, &mut r as *mut _ as *mut _, 0) != 0).then(|| {
            Rect::from_min_max(Pos2::new(r.l as f32, r.t as f32), Pos2::new(r.r as f32, r.b as f32))
        })
    }
}
/// Posição da janela (em pontos) já na criação, para não piscar em outro lugar.
/// O canto superior esquerdo do chat vai no canto superior esquerdo da bolha;
/// se não couber, encaixa dentro da área de trabalho (acima da barra do Windows).
fn posicao_janela() -> Option<Pos2> {
    let w = area_trabalho()?;
    let esc = unsafe { GetDpiForSystem() }.max(96) as f32 / 96.0;
    let (lw, lh, m) = (LARG * esc, ALT * esc, MARGEM * esc);
    let (x, y) = match perto() {
        Some([bx, by, _, _]) => (bx, by),
        None => (w.max.x, w.max.y), // sem bolha: canto inferior direito
    };
    let x = x.min(w.max.x - lw - m).max(w.min.x + m);
    let y = y.min(w.max.y - lh - m).max(w.min.y + m);
    Some(Pos2::new(x / esc, y / esc))
}
fn arredondar() {
    unsafe {
        let h = FindWindowW(std::ptr::null(), wide(TITULO_UI).as_ptr());
        if h != 0 {
            let p: u32 = 2;
            DwmSetWindowAttribute(h, 33, &p as *const u32 as *const _, 4);
        }
    }
}
fn modificado() -> Option<SystemTime> {
    std::fs::metadata(arquivo_conversa(&agora())).and_then(|m| m.modified()).ok()
}

struct App {
    conversa: Vec<Msg>,
    entrada: String,
    /// altura da faixa de digitação (acompanha a caixa: 3 a 4 linhas, depois rola)
    alt_entrada: f32,
    lider: Modelo,
    /// há turnos em andamento (mensagens mandadas e ainda sem resposta final)
    ocupado: bool,
    tx: Sender<Evento>,
    rx: Receiver<Evento>,
    ctx: egui::Context,
    /// sessão contínua do Claude Code (None = fechada; abre sozinha quando precisa)
    sessao: Option<Sessao>,
    /// a sessão acabou de nascer: a primeira mensagem leva o contexto do Claudinho
    sessao_nova: bool,
    pendentes: usize,
    tarefas: usize,
    /// Haiku pediu para passar a conversa (modelo, motivo) — aplica no fim do turno
    escalar: Option<(Modelo, String)>,
    /// o Opus devolveu para o modelo-base — aplica no fim do turno
    devolver: bool,
    /// rolagem automática do botão do meio: (ponto de âncora, quando apertou)
    autoscroll: Option<(Pos2, Instant)>,
    /// último pedido do usuário (reenviado numa sessão limpa quando o assunto muda)
    ultimo_pedido: String,
    /// o modelo marcou [[NOVO_ASSUNTO]] — limpa e reenvia no fim do turno
    novo_assunto: bool,
    /// o turno em andamento é o primeiro de uma sessão limpa (não aceita [[NOVO_ASSUNTO]])
    primeiro_turno: bool,
    /// tamanho (tokens) da sessão no último turno; 0 = sessão limpa ou desconhecida
    contexto_sessao: usize,
    /// o turno em andamento já usou alguma ferramenta (não dá para refazer sem repetir ação)
    ferramenta_no_turno: bool,
    /// assunto novo detectado com o turno já agindo: limpa quando ele terminar
    limpar_depois: bool,
    ultimo_envio: Option<(String, Instant)>,
    quadros: u32,
    focar: bool,
    pisca_ate: Option<Instant>,
    versao_arquivo: Option<SystemTime>,
    ultima_leitura: Instant,
    /// quantas mensagens recentes a tela mostra (a conversa inteira fica nos arquivos)
    limite: usize,
    tem_mais: bool,
    oculta: bool,
    focada_antes: bool,
    /// o que o Claude está fazendo agora
    progresso: Option<String>,
    /// pedido de aprovação aguardando o usuário
    permissao: Option<(String, String, Sender<bool>)>,
}

/// A tela carrega só o fim da conversa; "mostrar antigas" aumenta de 150 em 150.
const PAGINA: usize = 150;
/// Limpa a sessão sozinho quando o contexto passa disto (o mínimo fixo do Claude Code é ~53 mil).
const LIMITE_CONTEXTO: usize = 85_000;
/// Limpa a sessão sozinho quando a mensagem chega depois de tanto tempo sem conversa.
const PAUSA_LIMPA_MIN: i64 = 60;
/// Modelo que conduz a conversa por padrão (o Opus é chamado quando precisa e devolve para ele).
const BASE: Modelo = Modelo::Sonnet;
/// Registros antigos com o Haiku no comando passam a valer como o modelo-base.
fn base_se_haiku(m: Modelo) -> Modelo {
    if m == Modelo::Haiku { BASE } else { m }
}

/// Minutos desde uma mensagem (outro dia = muito tempo).
fn minutos_desde(m: &Msg) -> i64 {
    let t = agora();
    if m.data != data_iso(&t) {
        return i64::MAX;
    }
    let (h, mi) = m.hora.split_once(':').unwrap_or(("0", "0"));
    let antes = h.parse::<i64>().unwrap_or(0) * 60 + mi.parse::<i64>().unwrap_or(0);
    t.hora as i64 * 60 + t.min as i64 - antes
}

impl App {
    fn novo(piscar: bool, oculta: bool, ctx: egui::Context) -> Self {
        let (tx, rx) = channel();
        let (conversa, tem_mais) = carregar_ultimas(PAGINA);
        let lider = base_se_haiku(lider_atual(&conversa));
        App {
            ctx,
            sessao: None,
            sessao_nova: false,
            pendentes: 0,
            tarefas: 0,
            escalar: None,
            devolver: false,
            autoscroll: None,
            ultimo_pedido: String::new(),
            novo_assunto: false,
            primeiro_turno: false,
            contexto_sessao: sessao::contexto_salvo(),
            ferramenta_no_turno: false,
            limpar_depois: false,
            ultimo_envio: None,
            limite: PAGINA,
            tem_mais,
            oculta,
            focada_antes: false,
            progresso: None,
            permissao: None,
            conversa,
            entrada: String::new(),
            alt_entrada: 92.0,
            lider,
            ocupado: false,
            tx,
            rx,
            quadros: 0,
            focar: true,
            pisca_ate: piscar.then(|| Instant::now() + Duration::from_secs(6)),
            versao_arquivo: modificado(),
            ultima_leitura: Instant::now(),
        }
    }

    /// Garante a sessão do Claude Code aberta (retoma a salva; se não houver, cria uma nova).
    fn garantir_sessao(&mut self) -> bool {
        if let Some(s) = self.sessao.as_mut() {
            if s.viva() {
                return true;
            }
            self.sessao = None;
        }
        let c = self.ctx.clone();
        let acordar: std::sync::Arc<dyn Fn() + Send + Sync> = std::sync::Arc::new(move || c.request_repaint());
        let (id, retomar) = match sessao::id_salvo() {
            Some(id) => (id, true),
            None => (sessao::uuid4(), false),
        };
        match Sessao::abrir(&id, retomar, self.lider, self.tx.clone(), acordar) {
            Ok(s) => {
                if !retomar {
                    sessao::salvar_id(&id);
                    self.sessao_nova = true;
                }
                self.sessao = Some(s);
                true
            }
            Err(e) => {
                self.conversa.push(gravar("sistema", self.lider, &format!("⚠️ {e}")));
                false
            }
        }
    }

    /// Manda um texto para a sessão (com carimbo de hora e, se for o começo, o contexto).
    fn mandar_para_sessao(&mut self, texto: &str) {
        if !self.garantir_sessao() {
            return;
        }
        let mut corpo = String::new();
        self.primeiro_turno = std::mem::take(&mut self.sessao_nova);
        if self.primeiro_turno {
            corpo.push_str(&contexto_inicio_sessao());
        }
        corpo.push_str(&format!("{} {}", carimbo(), texto));
        if let Some(s) = self.sessao.as_mut() {
            s.enviar_texto(&corpo);
        }
        self.pendentes += 1;
        self.ocupado = true;
    }

    fn enviar(&mut self, _ctx: &egui::Context) {
        let t = self.entrada.trim().to_string();
        if t.is_empty() {
            return;
        }
        // Enter duplo / clique duplo: não manda a mesma coisa duas vezes
        if let Some((u, quando)) = &self.ultimo_envio {
            if *u == t && quando.elapsed() < Duration::from_secs(5) {
                self.entrada.clear();
                return;
            }
        }
        self.ultimo_envio = Some((t.clone(), Instant::now()));
        self.entrada.clear();
        // /clear: sessão nova do Claude Code (a memória longa continua)
        if t == "/clear" || t == "/limpar" {
            if let Some(s) = self.sessao.take() {
                s.encerrar();
            }
            sessao::esquecer_id();
            gravar("limpar", self.lider, "");
            if self.lider != BASE {
                gravar("lider", BASE, "");
            }
            self.conversa.push(gravar("sistema", BASE, "Conversa limpa: sessão nova do Claude (a memória longa continua)."));
            self.lider = BASE;
            self.pendentes = 0;
            self.ocupado = false;
            self.versao_arquivo = modificado();
            return;
        }
        // alertas do vigia desde a última mensagem dele entram como contexto
        let alertas: Vec<String> = self
            .conversa
            .iter()
            .rev()
            .take_while(|m| m.papel != "eu")
            .filter(|m| m.papel == "alerta")
            .map(|m| format!("- [{}] {}", m.hora, m.texto))
            .collect();
        // muito tempo sem conversa: o cache já expirou e o assunto quase sempre é outro
        let parado = self
            .conversa
            .iter()
            .rev()
            .find(|m| m.papel == "eu" || m.papel == "claude")
            .is_some_and(|m| minutos_desde(m) >= PAUSA_LIMPA_MIN);
        if parado && self.pendentes == 0 && self.tarefas == 0 && !t.starts_with('/') && (self.sessao.is_some() || sessao::id_salvo().is_some()) {
            self.reiniciar(&format!("mais de {} min sem conversa", PAUSA_LIMPA_MIN));
        }
        self.conversa.push(gravar("eu", self.lider, &t));
        self.versao_arquivo = modificado();
        let t_original = t.clone();
        let texto = if alertas.is_empty() || t.starts_with('/') {
            t
        } else {
            format!("{t}\n\n(Alertas que o vigia do Claudinho mostrou desde a minha última mensagem:\n{})", alertas.into_iter().rev().collect::<Vec<_>>().join("\n"))
        };
        self.ultimo_pedido = texto.clone();
        // sessão já com histórico: em paralelo ao envio, o Sonnet (sem ferramentas, ~2 mil tokens)
        // diz se o assunto mudou. Se mudou, o turno é refeito numa sessão limpa (ver Evento::Assunto).
        self.ferramenta_no_turno = false;
        self.limpar_depois = false;
        if self.contexto_sessao > 0 && self.pendentes == 0 && self.tarefas == 0 && !texto.starts_with('/') {
            let anteriores: Vec<String> = self
                .conversa
                .iter()
                .rev()
                .skip(1) // a mensagem que acabou de entrar
                .take_while(|m| m.papel != "limpar" && !(m.papel == "sistema" && m.texto.starts_with("🧹")))
                .filter(|m| m.papel == "eu" || m.papel == "claude")
                .take(6)
                .map(|m| format!("{}: {}", if m.papel == "eu" { nome() } else { "Claude" }, m.texto.chars().take(300).collect::<String>()))
                .collect();
            if !anteriores.is_empty() {
                let anteriores = anteriores.into_iter().rev().collect::<Vec<_>>().join("\n");
                let nova = t_original.clone();
                let tx = self.tx.clone();
                let c = self.ctx.clone();
                std::thread::spawn(move || {
                    let _ = tx.send(Evento::Assunto(assunto_novo(&anteriores, &nova)));
                    c.request_repaint();
                });
            }
        }
        self.mandar_para_sessao(&texto);
    }

    /// Limpeza automática: fecha a sessão do Claude Code e a próxima mensagem abre uma nova,
    /// só com a memória longa e as últimas mensagens (sem o histórico pesado). Volta para o modelo-base (Sonnet).
    fn reiniciar(&mut self, motivo: &str) {
        if let Some(s) = self.sessao.take() {
            s.encerrar();
        }
        sessao::esquecer_id();
        self.escalar = None;
        self.devolver = false;
        self.contexto_sessao = 0;
        if self.lider != BASE {
            gravar("lider", BASE, "");
            self.lider = BASE;
        }
        self.conversa.push(gravar("sistema", BASE, &format!("🧹 Conversa limpa sozinha ({motivo}). A memória longa continua.")));
        self.versao_arquivo = modificado();
    }

    fn parar(&mut self) {
        if let Some(s) = self.sessao.as_mut() {
            s.interromper();
        }
        self.conversa.push(gravar("sistema", self.lider, "⏹ Parado"));
        self.versao_arquivo = modificado();
    }

    fn receber(&mut self) {
        while let Ok(ev) = self.rx.try_recv() {
            match ev {
                Evento::Texto { modelo, texto } => {
                    let primeira = texto.lines().next().unwrap_or("").trim().to_string();
                    if primeira.starts_with("[[NOVO_ASSUNTO]]") {
                        // na sessão recém-limpa a marca não vale (evita laço); o resto do texto segue
                        if !self.primeiro_turno && !self.ultimo_pedido.is_empty() {
                            self.novo_assunto = true;
                            continue;
                        }
                        let resto = texto.replacen("[[NOVO_ASSUNTO]]", "", 1);
                        let resto = tirar_lembretes(resto.trim());
                        if !resto.is_empty() {
                            self.conversa.push(gravar("claude", modelo, &resto));
                        }
                        continue;
                    }
                    if self.novo_assunto {
                        continue;
                    }
                    if self.lider == Modelo::Haiku && (primeira.starts_with("[[SONNET]]") || primeira.starts_with("[[OPUS]]")) {
                        let novo = if primeira.starts_with("[[OPUS]]") { Modelo::Opus } else { Modelo::Sonnet };
                        let motivo = primeira.trim_start_matches("[[OPUS]]").trim_start_matches("[[SONNET]]").trim().to_string();
                        self.escalar = Some((novo, motivo));
                        continue;
                    }
                    if self.lider == Modelo::Sonnet && primeira.starts_with("[[OPUS]]") {
                        self.escalar = Some((Modelo::Opus, primeira.trim_start_matches("[[OPUS]]").trim().to_string()));
                        continue;
                    }
                    let mut t = texto.clone();
                    // devolução ao modelo-base: [[SONNET]] vindo do Opus (ou o antigo [[HAIKU]])
                    for marca in ["[[SONNET]]", "[[HAIKU]]"] {
                        if primeira.starts_with(marca) {
                            t = t.replacen(marca, "", 1);
                            if self.lider != BASE {
                                self.devolver = true;
                            }
                            break;
                        }
                    }
                    let t = tirar_lembretes(t.trim());
                    if !t.is_empty() {
                        self.conversa.push(gravar("claude", modelo, &t));
                    }
                }
                Evento::Progresso(p) => {
                    if self.pendentes > 0 {
                        self.ferramenta_no_turno = true;
                    }
                    self.progresso = Some(p);
                }
                Evento::Tarefas(n) => self.tarefas = n,
                Evento::Permissao { ferramenta, resumo, resposta } => {
                    self.permissao = Some((ferramenta, resumo, resposta));
                    self.pisca_ate = Some(Instant::now() + Duration::from_secs(4));
                    // se o chat estava recolhido na bolha, reaparece para ele decidir
                    unsafe {
                        let h = FindWindowW(std::ptr::null(), wide(TITULO_UI).as_ptr());
                        if h != 0 {
                            ShowWindow(h, 5); // SW_SHOW
                            SetForegroundWindow(h);
                        }
                    }
                }
                Evento::FimTurno { erro } => {
                    self.pendentes = self.pendentes.saturating_sub(1);
                    if let Some(e) = erro {
                        if !e.contains("interrupt") {
                            self.conversa.push(gravar("sistema", self.lider, &format!("⚠️ {}", e.chars().take(300).collect::<String>())));
                        }
                    }
                    if std::mem::take(&mut self.novo_assunto) {
                        // assunto novo: sessão limpa e o mesmo pedido vai de novo, já sem o histórico
                        self.pendentes = 0;
                        self.reiniciar("assunto novo");
                        let pedido = self.ultimo_pedido.clone();
                        self.mandar_para_sessao(&pedido);
                        continue;
                    }
                    if let Some((novo, motivo)) = self.escalar.take() {
                        let antes = self.lider;
                        // passa a conversa para o modelo maior e pede que ele responda
                        self.conversa.push(gravar(
                            "sistema",
                            self.lider,
                            &format!("{} chamou o {}{}", self.lider.nome(), novo.nome(), if motivo.is_empty() { String::new() } else { format!(": {motivo}") }),
                        ));
                        if let Some(s) = self.sessao.as_mut() {
                            s.trocar_modelo(novo);
                        }
                        self.lider = novo;
                        gravar("lider", novo, "");
                        self.mandar_para_sessao(&format!(
                            "[Nota do app Claudinho: o {} passou a conversa para você, Claude {}, porque: {motivo}. Agora responda/faça você o último pedido de {}. Ao terminar, devolva com [[SONNET]].]",
                            antes.nome(),
                            novo.nome(),
                            nome()
                        ));
                    } else if std::mem::take(&mut self.devolver) {
                        if let Some(s) = self.sessao.as_mut() {
                            s.trocar_modelo(BASE);
                        }
                        self.lider = BASE;
                        gravar("lider", BASE, "");
                    }
                    if self.pendentes == 0 {
                        self.ocupado = false;
                        self.focar = true;
                        self.progresso = None;
                        self.permissao = None;
                        // histórico pesado demais: cada mensagem relê tudo, então começa limpa na próxima
                        let tamanho = self.sessao.as_ref().map(|s| s.contexto.load(std::sync::atomic::Ordering::Relaxed)).unwrap_or(0);
                        if tamanho > 0 {
                            self.contexto_sessao = tamanho;
                            sessao::salvar_contexto(tamanho);
                        }
                        if std::mem::take(&mut self.limpar_depois) && self.tarefas == 0 {
                            self.reiniciar("assunto novo");
                        } else if tamanho > LIMITE_CONTEXTO && self.tarefas == 0 && self.escalar.is_none() {
                            self.reiniciar(&format!("conversa passou de {} mil tokens", tamanho / 1000));
                        }
                    }
                }
                Evento::Assunto(novo) => {
                    if !novo {
                        continue;
                    }
                    if self.pendentes == 0 {
                        // o turno já acabou (respondeu com o histórico velho): a próxima já vem limpa
                        self.reiniciar("assunto novo");
                    } else if !self.ferramenta_no_turno {
                        // ainda só pensando: interrompe e refaz o pedido numa sessão limpa
                        self.novo_assunto = true;
                        if let Some(s) = self.sessao.as_mut() {
                            s.interromper();
                        }
                    } else {
                        // já agiu (ferramenta): deixa terminar para não repetir ação; limpa depois
                        self.limpar_depois = true;
                    }
                }
                Evento::Encerrou { id, erro } => {
                    // fim de uma sessão antiga (limpa/fechada) não mexe na sessão atual
                    if self.sessao.as_ref().is_some_and(|s| s.id != id) {
                        continue;
                    }
                    let tinha_trabalho = self.pendentes > 0;
                    self.sessao = None;
                    self.pendentes = 0;
                    self.ocupado = false;
                    self.progresso = None;
                    self.permissao = None;
                    if let Some(e) = erro {
                        // sessão salva sumiu: começa outra (com o contexto do Claudinho)
                        if e.contains("No conversation found") || e.contains("not found") {
                            sessao::esquecer_id();
                        }
                        if tinha_trabalho {
                            self.conversa.push(gravar("sistema", self.lider, &format!("⚠️ O Claude Code fechou: {}", e.lines().last().unwrap_or(""))));
                        }
                    }
                }
            }
            self.versao_arquivo = modificado();
        }
        // sessão parada há 10 min, sem agentes rodando: fecha para devolver ~235 MB (retoma depois)
        if let Some(s) = self.sessao.as_ref() {
            if self.pendentes == 0 && self.tarefas == 0 && self.permissao.is_none() && s.parada_ha() > Duration::from_secs(10 * 60) {
                if let Some(s) = self.sessao.take() {
                    s.encerrar();
                }
            }
        }
        // alertas que o vigia gravou enquanto a janela está aberta
        if !self.ocupado && self.ultima_leitura.elapsed() > Duration::from_secs(3) {
            self.ultima_leitura = Instant::now();
            let v = modificado();
            if v != self.versao_arquivo {
                self.versao_arquivo = v;
                let antes = self.conversa.last().map(|m| (m.data.clone(), m.hora.clone(), m.texto.clone()));
                self.recarregar();
                let depois = self.conversa.last().map(|m| (m.data.clone(), m.hora.clone(), m.texto.clone()));
                if antes != depois && self.conversa.last().is_some_and(|m| m.papel == "alerta") {
                    self.pisca_ate = Some(Instant::now() + Duration::from_secs(5));
                }
            }
        }
    }

    fn recarregar(&mut self) {
        let (c, mais) = carregar_ultimas(self.limite);
        self.conversa = c;
        self.tem_mais = mais;
        self.lider = base_se_haiku(lider_atual(&self.conversa));
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
fn forte(t: f32) -> FontId {
    FontId::new(t, egui::FontFamily::Name("forte".into()))
}
fn botao_icone(ui: &mut egui::Ui, centro: Pos2, simbolo: &str, id: &str) -> bool {
    let r = Rect::from_center_size(centro, Vec2::splat(32.0));
    let resp = ui.interact(r, egui::Id::new(id), Sense::click());
    if resp.hovered() {
        ui.painter().circle_filled(r.center(), 16.0, CHIP);
    }
    ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, simbolo, FontId::proportional(24.0), if resp.hovered() { TEXTO } else { SUAVE });
    resp.clicked()
}

impl eframe::App for App {
    fn clear_color(&self, _: &egui::Visuals) -> [f32; 4] {
        egui::Rgba::from(BG).to_array()
    }

    fn ui(&mut self, raiz: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx_dono = raiz.ctx().clone();
        let ctx = &ctx_dono;
        self.quadros += 1;
        self.receber();
        ctx.request_repaint_after(Duration::from_secs(3));

        if self.quadros == 1 {
            if !self.oculta {
                ctx.send_viewport_cmd(ViewportCommand::Visible(true));
                ctx.send_viewport_cmd(ViewportCommand::Focus);
            }
            arredondar();
        }
        // revelada pelo vigia (estava pré-carregada): atualiza e põe o cursor no campo
        let focada = ctx.input(|i| i.viewport().focused.unwrap_or(false));
        if focada && !self.focada_antes {
            self.focar = true;
            if !self.ocupado {
                self.recarregar();
                self.versao_arquivo = modificado();
            }
            // já vai abrindo o Claude Code enquanto ele digita (a 1ª resposta sai mais rápido)
            if self.sessao.is_none() {
                self.garantir_sessao();
            }
        }
        self.focada_antes = focada;

        let piscando = self.pisca_ate.map(|t| Instant::now() < t).unwrap_or(false);
        if piscando {
            ctx.request_repaint_after(Duration::from_millis(60));
        }
        let borda = if piscando {
            let f = (ctx.input(|i| i.time) * 4.0).sin().abs() as f32;
            let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * f) as u8;
            Color32::from_rgb(mix(0xDD, 0x1E), mix(0xD8, 0x9C), mix(0xCA, 0x8C))
        } else {
            BORDA
        };

        if ctx.input(|i| i.key_pressed(Key::Escape)) && !recolher() {
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }
        let enter = ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter));

        let painel = egui::Frame::NONE.fill(BG).stroke(Stroke::new(if piscando { 2.0 } else { 1.0 }, borda));
        egui::CentralPanel::default().frame(painel).show(raiz, |ui| {
            let cheio = ui.max_rect();

            // cabeçalho arrastável
            let cab = Rect::from_min_size(cheio.min, Vec2::new(cheio.width(), 52.0));
            if ui.interact(cab, egui::Id::new("cab"), Sense::click_and_drag()).drag_started() {
                ctx.send_viewport_cmd(ViewportCommand::StartDrag);
            }
            {
                let p = ui.painter();
                logo(p, Pos2::new(cab.min.x + 26.0, cab.center().y), 12.0);
                p.text(Pos2::new(cab.min.x + 48.0, cab.center().y - 8.0), egui::Align2::LEFT_CENTER, "Claudinho", forte(16.0), TEXTO);
                let status = if self.ocupado { format!("{} pensando…", self.lider.nome()) } else { self.lider.nome().to_string() };
                p.text(Pos2::new(cab.min.x + 48.0, cab.center().y + 10.0), egui::Align2::LEFT_CENTER, status, FontId::proportional(11.0), SUAVE);
                p.line_segment([Pos2::new(cab.min.x, cab.max.y), cab.max], Stroke::new(1.0, BORDA));
            }
            // um só botão: fecha o chat (recolhe para a bolha, como o Esc)
            if botao_icone(ui, Pos2::new(cab.max.x - 24.0, cab.center().y), "×", "fechar") {
                if !recolher() {
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                }
            }

            // conversa
            // altura da faixa de digitação: acompanha a caixa (3 a 4 linhas; depois rola)
            let entrada_alt = self.alt_entrada;
            let lista = Rect::from_min_max(Pos2::new(cheio.min.x, cab.max.y + 1.0), Pos2::new(cheio.max.x, cheio.max.y - entrada_alt));
            // botão do meio = rolagem automática do Windows: clica e solta (fica ativa até outro
            // clique) ou segura e arrasta (para ao soltar); a velocidade segue a distância da âncora
            let (meio_down, meio_up, outro_clique, ponteiro) = ctx.input(|i| {
                (
                    i.pointer.button_pressed(egui::PointerButton::Middle),
                    i.pointer.button_released(egui::PointerButton::Middle),
                    i.pointer.button_pressed(egui::PointerButton::Primary) || i.pointer.button_pressed(egui::PointerButton::Secondary) || i.key_pressed(egui::Key::Escape),
                    i.pointer.hover_pos(),
                )
            });
            if meio_down {
                self.autoscroll = match (self.autoscroll, ponteiro) {
                    (None, Some(p)) if lista.contains(p) => Some((p, Instant::now())),
                    _ => None,
                };
            } else if outro_clique {
                self.autoscroll = None;
            } else if meio_up {
                if let (Some((ancora, t)), Some(p)) = (self.autoscroll, ponteiro) {
                    if t.elapsed() > Duration::from_millis(350) && (p.y - ancora.y).abs() > 12.0 {
                        self.autoscroll = None;
                    }
                }
            }
            let mut delta_auto = 0.0;
            if let Some((ancora, _)) = self.autoscroll {
                let dy = ponteiro.map(|p| p.y - ancora.y).unwrap_or(0.0);
                let fora = (dy.abs() - 10.0).max(0.0) * dy.signum();
                delta_auto = -fora * 0.25 * (1.0 + fora.abs() / 120.0);
                ctx.set_cursor_icon(if fora > 0.0 { egui::CursorIcon::ResizeSouth } else if fora < 0.0 { egui::CursorIcon::ResizeNorth } else { egui::CursorIcon::AllScroll });
                let p = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("autoscroll")));
                p.circle_filled(ancora, 9.0, CHIP);
                p.circle_stroke(ancora, 9.0, Stroke::new(1.0, SUAVE));
                p.text(ancora, egui::Align2::CENTER_CENTER, "↕", egui::FontId::proportional(11.0), SUAVE);
                ctx.request_repaint();
            }
            let mut lista_ui = ui.new_child(egui::UiBuilder::new().max_rect(lista.shrink2(Vec2::new(12.0, 6.0))));
            egui::ScrollArea::vertical().stick_to_bottom(true).auto_shrink([false, false]).show(&mut lista_ui, |ui| {
                if delta_auto != 0.0 {
                    ui.scroll_with_delta(Vec2::new(0.0, delta_auto));
                }
                ui.spacing_mut().item_spacing.y = 8.0;
                ui.add_space(4.0);
                if self.conversa.iter().all(|m| m.papel == "lider" || m.papel == "limpar") {
                    ui.label(RichText::new(format!("Oi, {}! Estou de olho no seu dia. Pergunte qualquer coisa ou peça um lembrete.", nome())).size(13.0).color(SUAVE));
                }
                let larg = ui.available_width();
                if self.tem_mais {
                    let mut carregar = false;
                    ui.vertical_centered(|ui| {
                        let b = egui::Button::new(RichText::new("↑ mostrar mensagens antigas").size(11.5).color(SUAVE)).fill(CHIP).corner_radius(CornerRadius::same(10));
                        carregar = ui.add(b).clicked();
                    });
                    if carregar {
                        self.limite += PAGINA;
                        self.recarregar();
                    }
                }
                let hoje = data_iso(&agora());
                let mut dia_ant = String::new();
                for m in self.conversa.iter().filter(|m| m.papel != "lider" && m.papel != "limpar") {
                    // separador quando muda o dia
                    if m.data != dia_ant {
                        dia_ant = m.data.clone();
                        let rot = if m.data == hoje { "Hoje".to_string() } else { m.quando().split(' ').next().unwrap_or("").to_string() };
                        ui.vertical_centered(|ui| {
                            ui.label(RichText::new(rot).size(10.5).color(SUAVE).strong());
                        });
                    }
                    if m.papel == "sistema" {
                        ui.vertical_centered(|ui| {
                            ui.label(RichText::new(format!("{} · {}", m.hora, m.texto)).size(11.0).color(SUAVE).italics());
                        });
                        continue;
                    }
                    let eu = m.papel == "eu";
                    let (fill, borda) = if eu { (BOLHA_EU, BOLHA_EU) } else if m.papel == "alerta" { (CAMPO, ACENTO) } else { (CAMPO, BORDA) };
                    let layout = if eu { Layout::right_to_left(Align::Min) } else { Layout::left_to_right(Align::Min) };
                    ui.with_layout(layout, |ui| {
                        egui::Frame::NONE
                            .fill(fill)
                            .stroke(Stroke::new(1.0, borda))
                            .corner_radius(CornerRadius::same(10))
                            .inner_margin(Margin::symmetric(10, 7))
                            .show(ui, |ui| {
                                ui.set_max_width(larg * 0.82);
                                ui.vertical(|ui| {
                                    if !eu {
                                        let rot = if m.papel == "alerta" { format!("⏰ {} · {}", m.modelo, m.hora) } else { format!("{} · {}", m.modelo, m.hora) };
                                        ui.label(RichText::new(rot).size(10.5).color(ACENTO));
                                    }
                                    if eu {
                                        ui.label(RichText::new(m.texto.replace('\u{FE0F}', "")).size(13.5).color(TEXTO));
                                    } else {
                                        markdown(ui, &m.texto);
                                    }
                                    if eu {
                                        ui.label(RichText::new(&m.hora).size(10.0).color(SUAVE));
                                    }
                                });
                            });
                    });
                }
                // pedido de aprovação, como no Claude Code
                let mut decisao: Option<bool> = None;
                if let Some((ferr, resumo, _)) = &self.permissao {
                    egui::Frame::NONE
                        .fill(CAMPO)
                        .stroke(Stroke::new(1.5, ACENTO))
                        .corner_radius(CornerRadius::same(10))
                        .inner_margin(Margin::symmetric(10, 8))
                        .show(ui, |ui| {
                            ui.set_width(larg - 22.0);
                            ui.label(RichText::new(format!("🔐 O Claude quer usar {ferr}")).font(forte(13.0)).color(TEXTO));
                            ui.label(RichText::new(resumo).monospace().size(11.5).color(TEXTO));
                            ui.add_space(4.0);
                            ui.horizontal(|ui| {
                                let sim = egui::Button::new(RichText::new("Permitir").font(forte(13.0)).color(Color32::WHITE)).fill(ACENTO).corner_radius(CornerRadius::same(8));
                                if ui.add(sim).clicked() {
                                    decisao = Some(true);
                                }
                                let nao = egui::Button::new(RichText::new("Negar").size(13.0).color(TEXTO)).fill(CHIP).corner_radius(CornerRadius::same(8));
                                if ui.add(nao).clicked() {
                                    decisao = Some(false);
                                }
                            });
                        });
                }
                if let Some(ok) = decisao {
                    if let Some((ferr, resumo, tx)) = self.permissao.take() {
                        let _ = tx.send(ok);
                        let primeira = resumo.lines().next().unwrap_or("").chars().take(80).collect::<String>();
                        self.conversa.push(gravar("sistema", self.lider, &format!("{} {ferr}: {primeira}", if ok { "✅ Permitido" } else { "⛔ Negado" })));
                        self.versao_arquivo = modificado();
                    }
                }
                if self.ocupado && self.permissao.is_none() {
                    ui.horizontal(|ui| {
                        ui.add(egui::Spinner::new().size(14.0).color(ACENTO));
                        let txt = match &self.progresso {
                            Some(p) => format!("{}: {}", self.lider.nome(), p),
                            None => format!("{} pensando…", self.lider.nome()),
                        };
                        ui.label(RichText::new(txt).size(12.0).color(SUAVE));
                    });
                }
                ui.add_space(4.0);
            });

            // entrada
            let base = Rect::from_min_max(Pos2::new(cheio.min.x, cheio.max.y - entrada_alt), cheio.max);
            ui.painter().line_segment([base.min, Pos2::new(base.max.x, base.min.y)], Stroke::new(1.0, BORDA));
            let mut ent_ui = ui.new_child(egui::UiBuilder::new().max_rect(base.shrink2(Vec2::new(12.0, 10.0))));
            let mut alt_caixa = 0.0;
            ent_ui.horizontal(|ui| {
                // caixa com fundo fixo e texto rolando dentro: mostra até 4 linhas, o resto rola
                let linha = ui.text_style_height(&egui::TextStyle::Body);
                let max_texto = linha * 4.0 + 2.0;
                let larg = ui.available_width() - 86.0;
                let caixa = egui::Frame::NONE
                    .fill(CAMPO)
                    .stroke(Stroke::new(1.0, BORDA))
                    .corner_radius(CornerRadius::same(6))
                    .inner_margin(Margin::same(8))
                    .show(ui, |ui| {
                        ui.set_width(larg - 16.0);
                        egui::ScrollArea::vertical()
                            .id_salt("entrada")
                            .max_height(max_texto)
                            .auto_shrink([false, true])
                            .show(ui, |ui| {
                                let te = egui::TextEdit::multiline(&mut self.entrada)
                                    .hint_text(RichText::new("Escreva… (Enter envia · Esc fecha)").color(SUAVE))
                                    .desired_rows(3)
                                    .desired_width(f32::INFINITY)
                                    .text_color(TEXTO)
                                    .frame(egui::Frame::NONE)
                                    .margin(Margin::same(0));
                                ui.add(te)
                            })
                            .inner
                    });
                alt_caixa = caixa.response.rect.height();
                let r = caixa.inner;
                if self.focar {
                    r.request_focus();
                    self.focar = false;
                }
                // trabalhando e sem texto novo: o botão vira Parar (como o Esc do Claude Code)
                let parar = self.ocupado && self.entrada.trim().is_empty();
                let b = egui::Button::new(RichText::new(if parar { "Parar" } else { "Enviar" }).font(forte(14.0)).color(Color32::WHITE))
                    .fill(if parar { TEXTO } else { ACENTO })
                    .corner_radius(CornerRadius::same(8))
                    .min_size(Vec2::new(76.0, 34.0));
                if ui.add(b).clicked() {
                    if parar {
                        self.parar();
                    } else {
                        self.enviar(ctx);
                    }
                }
            });
            // a faixa cresce/encolhe com a caixa (vale a partir do próximo quadro)
            let nova_alt = (alt_caixa + 20.0).max(56.0);
            if (nova_alt - self.alt_entrada).abs() > 0.5 {
                self.alt_entrada = nova_alt;
                ctx.request_repaint();
            }
        });

        if enter {
            self.enviar(ctx);
        }
    }
}

/// Markdown leve para as respostas: títulos, listas, **negrito**, `código` e blocos ```.
fn markdown(ui: &mut egui::Ui, texto: &str) {
    use egui::text::{LayoutJob, TextFormat};
    // o seletor de emoji colorido (U+FE0F) não tem desenho e viraria um quadradinho
    let texto = &texto.replace('\u{FE0F}', "");
    let normal = TextFormat { font_id: FontId::proportional(13.5), color: TEXTO, ..Default::default() };
    let negrito = TextFormat { font_id: forte(13.5), color: TEXTO, ..Default::default() };
    let codigo = TextFormat { font_id: FontId::monospace(12.0), color: TEXTO, background: CHIP, ..Default::default() };
    let mut job = LayoutJob::default();
    let mut em_bloco = false;
    for (n, linha) in texto.lines().enumerate() {
        if n > 0 {
            job.append("\n", 0.0, normal.clone());
        }
        if linha.trim_start().starts_with("```") {
            em_bloco = !em_bloco;
            continue;
        }
        if em_bloco {
            job.append(linha, 0.0, codigo.clone());
            continue;
        }
        let l = linha.trim_end();
        let (l, base) = if let Some(r) = l.trim_start().strip_prefix('#') {
            (r.trim_start_matches('#').trim_start().to_string(), TextFormat { font_id: forte(14.5), color: TEXTO, ..Default::default() })
        } else if let Some(r) = l.trim_start().strip_prefix("- ").or_else(|| l.trim_start().strip_prefix("* ")) {
            let recuo = l.len() - l.trim_start().len();
            (format!("{}• {r}", " ".repeat(recuo)), normal.clone())
        } else if l.trim() == "---" {
            ("──────────".to_string(), TextFormat { color: SUAVE, ..normal.clone() })
        } else {
            (l.to_string(), normal.clone())
        };
        // trechos **negrito** e `código` dentro da linha
        let mut resto = l.as_str();
        while !resto.is_empty() {
            let b = resto.find("**");
            let c = resto.find('`');
            match (b, c) {
                (Some(i), _) if c.is_none_or(|j| i < j) => {
                    job.append(&resto[..i], 0.0, base.clone());
                    let depois = &resto[i + 2..];
                    if let Some(f) = depois.find("**") {
                        job.append(&depois[..f], 0.0, negrito.clone());
                        resto = &depois[f + 2..];
                    } else {
                        job.append(depois, 0.0, base.clone());
                        resto = "";
                    }
                }
                (_, Some(j)) => {
                    job.append(&resto[..j], 0.0, base.clone());
                    let depois = &resto[j + 1..];
                    if let Some(f) = depois.find('`') {
                        job.append(&depois[..f], 0.0, codigo.clone());
                        resto = &depois[f + 1..];
                    } else {
                        job.append(depois, 0.0, base.clone());
                        resto = "";
                    }
                }
                _ => {
                    job.append(resto, 0.0, base.clone());
                    resto = "";
                }
            }
        }
    }
    job.wrap.max_width = ui.available_width();
    ui.label(job);
}

fn fontes_windows(ctx: &egui::Context) {
    use egui::{FontData, FontDefinitions, FontFamily};
    let mut f = FontDefinitions::default();
    let dir = Path::new(r"C:\Windows\Fonts");
    // sem seguiemj.ttf (12 MB, atrasava a abertura): os emojis vêm das fontes embutidas do egui
    for (nome, arq) in [("segoe", "segoeui.ttf"), ("segoe-sb", "seguisb.ttf")] {
        if let Ok(b) = std::fs::read(dir.join(arq)) {
            f.font_data.insert(nome.into(), std::sync::Arc::new(FontData::from_owned(b)));
        }
    }
    let prop = f.families.entry(FontFamily::Proportional).or_default();
    let mut i = 0;
    for n in ["segoe", "emoji"] {
        if f.font_data.contains_key(n) {
            prop.insert(i, n.to_string());
            i += 1;
        }
    }
    // reserva para símbolos/emojis que faltam nas outras (⏰ ✍ 💪…), em versão monocromática (2,5 MB)
    if let Ok(b) = std::fs::read(dir.join("seguisym.ttf")) {
        f.font_data.insert("simbolos".into(), std::sync::Arc::new(FontData::from_owned(b)));
        f.families.entry(FontFamily::Proportional).or_default().push("simbolos".into());
    }
    let mut forte = f.families[&FontFamily::Proportional].clone();
    if f.font_data.contains_key("segoe-sb") {
        forte.insert(0, "segoe-sb".into());
    }
    f.families.insert(FontFamily::Name("forte".into()), forte);
    ctx.set_fonts(f);
}

fn perto() -> Option<[f32; 4]> {
    let a: Vec<String> = std::env::args().collect();
    let i = a.iter().position(|s| s == "--perto")?;
    let v: Vec<f32> = a.get(i + 1)?.split(',').filter_map(|s| s.trim().parse().ok()).collect();
    (v.len() == 4).then(|| [v[0], v[1], v[2], v[3]])
}

fn main() {
    unsafe {
        CreateMutexW(std::ptr::null(), 1, wide("Local\\ClaudinhoUI").as_ptr());
        if GetLastError() == 183 {
            if std::env::args().any(|a| a == "--oculta") {
                return; // já existe um chat carregado
            }
            let h = FindWindowW(std::ptr::null(), wide(TITULO_UI).as_ptr());
            if h != 0 {
                ShowWindow(h, 9);
                SetForegroundWindow(h);
            }
            return;
        }
    }
    let piscar = std::env::args().any(|a| a == "--piscar");
    let oculta = std::env::args().any(|a| a == "--oculta");
    migrar_conversas_antigas();
    let mut vp = egui::ViewportBuilder::default();
    if let Some(p) = posicao_janela() {
        vp = vp.with_position(p);
    }
    let vp = vp
        .with_title(TITULO_UI)
        .with_inner_size([LARG, ALT])
        .with_decorations(false)
        .with_always_on_top()
        .with_resizable(false)
        .with_taskbar(true)
        .with_icon(std::sync::Arc::new(egui::IconData { rgba: mascote_rgba(64), width: 64, height: 64 }))
        .with_visible(false)
        .with_active(true);
    let opts = eframe::NativeOptions { viewport: vp, ..Default::default() };
    let _ = eframe::run_native(
        "claudinho-ui",
        opts,
        Box::new(move |cc| {
            let mut v = egui::Visuals::light();
            v.panel_fill = BG;
            v.window_fill = BG;
            v.extreme_bg_color = CAMPO;
            v.selection.bg_fill = ACENTO.gamma_multiply(0.35);
            v.selection.stroke = Stroke::new(1.0, ACENTO);
            v.text_cursor.stroke = Stroke::new(2.0, ACENTO);
            v.widgets.inactive.bg_stroke = Stroke::new(1.0, BORDA);
            v.widgets.hovered.bg_stroke = Stroke::new(1.0, ACENTO);
            v.widgets.active.bg_stroke = Stroke::new(1.0, ACENTO);
            cc.egui_ctx.set_visuals(v);
            fontes_windows(&cc.egui_ctx);
            Ok(Box::new(App::novo(piscar, oculta, cc.egui_ctx.clone())))
        }),
    );
}
