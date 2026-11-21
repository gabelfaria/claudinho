//! Sessão contínua do Claude Code (igual a uma conversa do app): um processo `claude -p`
//! em stream-json que fica vivo entre as mensagens, com o prompt original do Claude Code
//! + as regras do Claudinho acrescentadas, rodando na pasta de trabalho, com Chrome, conectores,
//! agentes em segundo plano e aprovação pelo chat. Fecha sozinha quando fica parada e
//! continua de onde parou (`--resume`) na próxima mensagem.

use crate::comum::*;
use serde_json::{Value, json};
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::Instant;

pub enum Evento {
    /// texto do Claude (pode vir em partes durante o turno)
    Texto { modelo: Modelo, texto: String },
    /// o que ele está fazendo agora
    Progresso(String),
    /// aprovação: o app mostra Permitir/Negar e responde pelo canal
    Permissao { ferramenta: String, resumo: String, resposta: Sender<bool> },
    /// terminou um turno (respondeu)
    FimTurno { erro: Option<String> },
    /// quantos agentes/tarefas em segundo plano estão rodando
    Tarefas(usize),
    /// o processo terminou (normal ou caiu); texto do erro se houver
    Encerrou { id: String, erro: Option<String> },
    /// resultado da checagem de assunto feita antes de mandar a mensagem (true = assunto novo)
    Assunto(bool),
}

#[link(name = "user32")]
unsafe extern "system" {
    fn FindWindowW(classe: *const u16, titulo: *const u16) -> isize;
    fn ShowWindow(h: isize, cmd: i32) -> i32;
    fn SetForegroundWindow(h: isize) -> i32;
}
/// Traz o chat para frente (pedido de aprovação com a janela recolhida na bolha).
fn chamar_atencao() {
    unsafe {
        let h = FindWindowW(std::ptr::null(), wide(TITULO_UI).as_ptr());
        if h != 0 {
            ShowWindow(h, 5); // SW_SHOW
            SetForegroundWindow(h);
        }
    }
}

fn arquivo_sessao() -> PathBuf {
    pasta_dia().join("sessao.json")
}
pub fn id_salvo() -> Option<String> {
    let v: Value = serde_json::from_str(&std::fs::read_to_string(arquivo_sessao()).ok()?).ok()?;
    v["id"].as_str().map(String::from)
}
pub fn salvar_id(id: &str) {
    let _ = std::fs::create_dir_all(pasta_dia());
    let _ = std::fs::write(arquivo_sessao(), json!({"id": id, "desde": format!("{} {}", data_iso(&agora()), hhmm(&agora()))}).to_string());
}
pub fn esquecer_id() {
    let _ = std::fs::remove_file(arquivo_sessao());
    let _ = std::fs::remove_file(pasta_dia().join("sessao-contexto.txt"));
}
/// O Claude Code só grava a conversa (`~/.claude/projects/<pasta>/<id>.jsonl`) na primeira
/// mensagem. Sessão aberta e fechada sem mensagem não existe: `--resume` dela dá
/// "No conversation found" (error_during_execution).
pub fn transcricao_existe(id: &str) -> bool {
    let home = std::env::var("USERPROFILE").unwrap_or_default();
    let pasta: String = pasta_projetos().to_string_lossy().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    std::path::Path::new(&home).join(".claude").join("projects").join(pasta).join(format!("{id}.jsonl")).exists()
}
/// Qual sessão abrir: (id, retomar). Retoma a salva só se ela foi gravada de verdade;
/// senão cria uma nova (e o app manda o contexto do Claudinho na primeira mensagem).
pub fn escolher_sessao() -> (String, bool) {
    match id_salvo() {
        Some(id) if transcricao_existe(&id) => (id, true),
        _ => (uuid4(), false),
    }
}
/// Tamanho (tokens) da sessão salva no último turno, para sobreviver a um reinício do chat.
pub fn salvar_contexto(n: usize) {
    let _ = std::fs::write(pasta_dia().join("sessao-contexto.txt"), n.to_string());
}
pub fn contexto_salvo() -> usize {
    if id_salvo().is_none() {
        return 0;
    }
    std::fs::read_to_string(pasta_dia().join("sessao-contexto.txt")).ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0)
}

/// UUID v4 sem dependências extras.
pub fn uuid4() -> String {
    use std::hash::{BuildHasher, Hasher};
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0);
    let mut b = [0u8; 16];
    for i in 0..2 {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u64(t ^ (i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
        h.write_u32(std::process::id());
        b[i * 8..i * 8 + 8].copy_from_slice(&h.finish().to_le_bytes());
    }
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let x: String = b.iter().map(|v| format!("{v:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &x[0..8], &x[8..12], &x[12..16], &x[16..20], &x[20..32])
}

fn modelo_de_id(s: &str) -> Option<Modelo> {
    if s.contains("opus") {
        Some(Modelo::Opus)
    } else if s.contains("sonnet") {
        Some(Modelo::Sonnet)
    } else if s.contains("haiku") {
        Some(Modelo::Haiku)
    } else {
        None
    }
}

/// Texto curto do que a ferramenta quer fazer, para o cartão de aprovação.
pub fn resumo_ferramenta(nome: &str, input: &Value) -> String {
    let s = |k: &str| input[k].as_str().unwrap_or("").to_string();
    let r = match nome {
        "Bash" | "PowerShell" => {
            let d = s("description");
            if d.is_empty() { s("command") } else { format!("{d}\n{}", s("command")) }
        }
        "Edit" | "Write" | "Read" | "NotebookEdit" => s("file_path"),
        "WebFetch" => s("url"),
        "WebSearch" => s("query"),
        _ => {
            let t = input.to_string();
            if t == "{}" { String::new() } else { t }
        }
    };
    r.chars().take(600).collect()
}

/// Nome amigável da ferramenta para o progresso e o cartão.
pub fn nome_amigavel(nome: &str) -> String {
    if let Some(r) = nome.strip_prefix("mcp__") {
        let mut p = r.splitn(2, "__");
        let srv = p.next().unwrap_or("").replace("claude-in-chrome", "Chrome").replace("claude_ai_", "").replace('_', " ");
        let acao = p.next().unwrap_or("").replace('_', " ");
        return format!("{srv}: {acao}");
    }
    match nome {
        "Agent" | "Task" => "agente".into(),
        "WebSearch" => "busca na web".into(),
        "WebFetch" => "leitura de site".into(),
        "Edit" => "edição de arquivo".into(),
        "Write" => "criação de arquivo".into(),
        "Read" => "leitura de arquivo".into(),
        o => o.into(),
    }
}

pub struct Sessao {
    filho: Child,
    entrada: Arc<Mutex<Option<ChildStdin>>>,
    pub id: String,
    pub modelo: Modelo,
    pub ultima_atividade: Arc<Mutex<Instant>>,
    pub tarefas: Arc<AtomicUsize>,
    pub iniciou: Arc<AtomicBool>,
    /// tokens de contexto da última chamada da conversa principal (entrada + cache)
    pub contexto: Arc<AtomicUsize>,
    /// o usuário apertou parar: o fim do turno vem como error_during_execution, mas não é erro
    interrompido: Arc<AtomicBool>,
    seq: usize,
}

impl Sessao {
    /// Abre a sessão. `retomar` = continuar a sessão salva; senão cria uma nova com `id`.
    pub fn abrir(id: &str, retomar: bool, modelo: Modelo, eventos: Sender<Evento>, acordar: Arc<dyn Fn() + Send + Sync>) -> Result<Sessao, String> {
        use std::os::windows::process::CommandExt;
        let mut cmd = Command::new(claude_exe());
        cmd.current_dir(pasta_projetos())
            .args(["-p", "--model", modelo.alias(), "--effort", "medium"])
            .args(["--verbose", "--input-format", "stream-json", "--output-format", "stream-json"])
            .args(["--permission-mode", "auto", "--permission-prompts", "host", "--permission-prompt-tool", "stdio"])
            .arg("--chrome")
            .arg("--append-system-prompt-file")
            .arg(sistema_md());
        if retomar {
            cmd.args(["--resume", id]);
        } else {
            cmd.args(["--session-id", id]);
        }
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        let mut filho = cmd.spawn().map_err(|e| format!("não consegui abrir o Claude Code ({}): {e}", claude_exe().display()))?;
        let entrada = Arc::new(Mutex::new(filho.stdin.take()));
        let ultima = Arc::new(Mutex::new(Instant::now()));
        let tarefas = Arc::new(AtomicUsize::new(0));
        let iniciou = Arc::new(AtomicBool::new(false));
        let contexto = Arc::new(AtomicUsize::new(0));
        let interrompido = Arc::new(AtomicBool::new(false));

        // stderr guardado para explicar quedas
        let erros = Arc::new(Mutex::new(String::new()));
        if let Some(err) = filho.stderr.take() {
            let e2 = erros.clone();
            std::thread::spawn(move || {
                for l in std::io::BufReader::new(err).lines().map_while(Result::ok) {
                    let mut g = e2.lock().unwrap();
                    if g.len() < 4000 {
                        g.push_str(&l);
                        g.push('\n');
                    }
                }
            });
        }

        // leitor: transforma o stream-json em eventos para a janela
        if let Some(out) = filho.stdout.take() {
            let (ent, ult, tar, ini, ctxt, parou) = (entrada.clone(), ultima.clone(), tarefas.clone(), iniciou.clone(), contexto.clone(), interrompido.clone());
            let id_sessao = id.to_string();
            std::thread::spawn(move || {
                let mandar = |v: Value| {
                    if let Some(si) = ent.lock().unwrap().as_mut() {
                        let _ = writeln!(si, "{v}");
                        let _ = si.flush();
                    }
                };
                let enviar = |e: Evento| {
                    let _ = eventos.send(e);
                    acordar();
                };
                for linha in std::io::BufReader::new(out).lines().map_while(Result::ok) {
                    *ult.lock().unwrap() = Instant::now();
                    let Ok(v) = serde_json::from_str::<Value>(&linha) else { continue };
                    match v["type"].as_str().unwrap_or("") {
                        "system" => match v["subtype"].as_str().unwrap_or("") {
                            "init" => ini.store(true, Ordering::Relaxed),
                            "background_tasks_changed" => {
                                let n = v["tasks"].as_array().map(|a| a.len()).unwrap_or(0);
                                tar.store(n, Ordering::Relaxed);
                                enviar(Evento::Tarefas(n));
                            }
                            "task_started" => {
                                let d = v["description"].as_str().unwrap_or("tarefa");
                                enviar(Evento::Progresso(format!("agente em segundo plano: {d}")));
                            }
                            "compact_boundary" => enviar(Evento::Progresso("compactando a conversa…".into())),
                            _ => {}
                        },
                        "assistant" => {
                            // texto de subagente não aparece no chat
                            if !v["parent_tool_use_id"].is_null() {
                                continue;
                            }
                            let u = &v["message"]["usage"];
                            let n: u64 = ["input_tokens", "cache_creation_input_tokens", "cache_read_input_tokens"].iter().map(|k| u[*k].as_u64().unwrap_or(0)).sum();
                            if n > 0 {
                                ctxt.store(n as usize, Ordering::Relaxed);
                            }
                            let modelo =v["message"]["model"].as_str().and_then(modelo_de_id).unwrap_or(modelo);
                            if let Some(itens) = v["message"]["content"].as_array() {
                                for it in itens {
                                    match it["type"].as_str().unwrap_or("") {
                                        "text" => {
                                            let t = it["text"].as_str().unwrap_or("").trim().to_string();
                                            if !t.is_empty() {
                                                enviar(Evento::Texto { modelo, texto: t });
                                            }
                                        }
                                        "tool_use" => {
                                            let nome = it["name"].as_str().unwrap_or("ferramenta");
                                            let d = it["input"]["description"].as_str().unwrap_or("");
                                            enviar(Evento::Progresso(if d.is_empty() { format!("usando {}…", nome_amigavel(nome)) } else { format!("{d}…") }));
                                        }
                                        _ => {}
                                    }
                                }
                            }
                        }
                        "control_request" => {
                            let id = v["request_id"].clone();
                            let req = &v["request"];
                            if req["subtype"] == "can_use_tool" {
                                let nome = req["tool_name"].as_str().unwrap_or("?").to_string();
                                let input = req["input"].clone();
                                let (rtx, rrx) = std::sync::mpsc::channel::<bool>();
                                chamar_atencao();
                                enviar(Evento::Permissao { ferramenta: nome_amigavel(&nome), resumo: resumo_ferramenta(&nome, &input), resposta: rtx });
                                let ok = rrx.recv().unwrap_or(false);
                                let resp = if ok {
                                    json!({"behavior": "allow", "updatedInput": input})
                                } else {
                                    json!({"behavior": "deny", "message": format!("{} negou essa ação. Não tente contornar; diga em uma frase o que ficou faltando.", crate::comum::nome())})
                                };
                                mandar(json!({"type": "control_response", "response": {"subtype": "success", "request_id": id, "response": resp}}));
                            } else {
                                mandar(json!({"type": "control_response", "response": {"subtype": "error", "request_id": id, "error": "não suportado pelo Claudinho"}}));
                            }
                        }
                        "result" => {
                            let erro = if parou.swap(false, Ordering::Relaxed) {
                                None
                            } else if v["is_error"].as_bool().unwrap_or(false) {
                                let r = v["result"].as_str().unwrap_or("").to_string();
                                Some(if r.is_empty() { v["subtype"].as_str().unwrap_or("erro").to_string() } else { r })
                            } else {
                                None
                            };
                            enviar(Evento::FimTurno { erro });
                        }
                        _ => {}
                    }
                }
                let e = erros.lock().unwrap().trim().to_string();
                enviar(Evento::Encerrou { id: id_sessao, erro: if e.is_empty() { None } else { Some(e.chars().take(400).collect()) } });
            });
        }

        let s = Sessao { filho, entrada, id: id.to_string(), modelo, ultima_atividade: ultima, tarefas, iniciou, contexto, interrompido, seq: 0 };
        s.mandar(json!({"type": "control_request", "request_id": "ini", "request": {"subtype": "initialize"}}));
        Ok(s)
    }

    fn mandar(&self, v: Value) {
        if let Some(si) = self.entrada.lock().unwrap().as_mut() {
            let _ = writeln!(si, "{v}");
            let _ = si.flush();
        }
    }

    pub fn enviar_texto(&mut self, texto: &str) {
        *self.ultima_atividade.lock().unwrap() = Instant::now();
        self.interrompido.store(false, Ordering::Relaxed);
        self.mandar(json!({"type": "user", "session_id": "", "parent_tool_use_id": null, "message": {"role": "user", "content": texto}}));
    }

    pub fn trocar_modelo(&mut self, m: Modelo) {
        self.seq += 1;
        self.mandar(json!({"type": "control_request", "request_id": format!("m{}", self.seq), "request": {"subtype": "set_model", "model": m.alias()}}));
        self.modelo = m;
    }

    pub fn interromper(&mut self) {
        self.interrompido.store(true, Ordering::Relaxed);
        self.seq += 1;
        self.mandar(json!({"type": "control_request", "request_id": format!("i{}", self.seq), "request": {"subtype": "interrupt"}}));
    }

    pub fn viva(&mut self) -> bool {
        matches!(self.filho.try_wait(), Ok(None))
    }

    pub fn parada_ha(&self) -> std::time::Duration {
        self.ultima_atividade.lock().unwrap().elapsed()
    }

    /// Fecha a entrada (o Claude Code salva e sai); mata se não sair em 3 s.
    pub fn encerrar(mut self) {
        self.entrada.lock().unwrap().take();
        let t = Instant::now();
        while t.elapsed().as_secs() < 3 {
            if let Ok(Some(_)) = self.filho.try_wait() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let _ = self.filho.kill();
    }
}
