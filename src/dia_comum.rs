//! Lógica compartilhada do Claudinho (vigia residente e janela de conversa).
//! Conversa única (a vida toda do app) em <dados>\conversa\AAAA-MM.jsonl + cópia legível .md.
//! A IA vê só: memória longa compactada (memoria-longa.md) + mensagens ainda não resumidas.
#![allow(dead_code)]

use serde_json::{Value, json};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

pub const TITULO_UI: &str = "Claudinho";
/// Prompt padrão (copiado para a pasta do usuário na primeira vez, para ele editar).
const PROMPT_MODELO: &str = include_str!("../prompt/prompt-modelo.md");

// ---------- configuração ----------
/// `%APPDATA%\Claudinho\config.json` (ou o caminho em CLAUDINHO_CONFIG). Criado na primeira vez.
pub struct Config {
    /// como o assistente chama o usuário
    pub nome: String,
    /// pasta onde o Claude Code trabalha (lê/edita arquivos)
    pub pasta_trabalho: PathBuf,
    /// conversa, memória longa, agenda, lembretes
    pub pasta_dados: PathBuf,
    /// prompt do assistente (personalidade, regras, rotina)
    pub prompt: PathBuf,
}

fn env_ou(var: &str, padrao: &str) -> String {
    std::env::var(var).ok().filter(|s| !s.is_empty()).unwrap_or_else(|| padrao.to_string())
}

pub fn arquivo_config() -> PathBuf {
    if let Ok(p) = std::env::var("CLAUDINHO_CONFIG") {
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    Path::new(&env_ou("APPDATA", ".")).join("Claudinho").join("config.json")
}

pub fn config() -> &'static Config {
    static C: OnceLock<Config> = OnceLock::new();
    C.get_or_init(|| {
        let arq = arquivo_config();
        let base = arq.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."));
        let v: Value = std::fs::read_to_string(&arq).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(json!({}));
        let txt = |k: &str| v[k].as_str().filter(|s| !s.trim().is_empty()).map(String::from);
        let c = Config {
            nome: txt("nome").unwrap_or_else(|| env_ou("USERNAME", "você")),
            pasta_trabalho: txt("pasta_trabalho").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(env_ou("USERPROFILE", "."))),
            pasta_dados: txt("pasta_dados").map(PathBuf::from).unwrap_or_else(|| base.join("dados")),
            prompt: txt("prompt").map(PathBuf::from).unwrap_or_else(|| base.join("prompt.md")),
        };
        // primeira vez: grava o config e o prompt padrão para o usuário poder editar
        let _ = std::fs::create_dir_all(&base);
        if !arq.exists() {
            let _ = std::fs::write(
                &arq,
                serde_json::to_string_pretty(&json!({
                    "nome": c.nome,
                    "pasta_trabalho": c.pasta_trabalho,
                    "pasta_dados": c.pasta_dados,
                    "prompt": c.prompt,
                }))
                .unwrap_or_default(),
            );
        }
        if !c.prompt.exists() {
            let _ = std::fs::write(&c.prompt, PROMPT_MODELO);
        }
        let _ = std::fs::create_dir_all(&c.pasta_dados);
        c
    })
}

/// Nome do usuário, para os textos e prompts.
pub fn nome() -> &'static str {
    &config().nome
}

// ---------- hora local ----------
#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct SysTime {
    pub ano: u16,
    pub mes: u16,
    pub dia_semana: u16,
    pub dia: u16,
    pub hora: u16,
    pub min: u16,
    pub seg: u16,
    pub ms: u16,
}
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetLocalTime(t: *mut SysTime);
}
pub fn agora() -> SysTime {
    let mut t = SysTime::default();
    unsafe { GetLocalTime(&mut t) };
    t
}
pub const DIAS: [&str; 7] = ["domingo", "segunda", "terça", "quarta", "quinta", "sexta", "sábado"];
pub fn data_iso(t: &SysTime) -> String {
    format!("{:04}-{:02}-{:02}", t.ano, t.mes, t.dia)
}
pub fn hhmm(t: &SysTime) -> String {
    format!("{:02}:{:02}", t.hora, t.min)
}
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

// ---------- caminhos ----------
pub fn pasta_dia() -> PathBuf {
    config().pasta_dados.clone()
}
/// Pasta onde o Claude Code trabalha (cwd da sessão).
pub fn pasta_projetos() -> PathBuf {
    config().pasta_trabalho.clone()
}
/// Conversa única da vida do app, em fatias mensais (só acrescenta, nunca reescreve).
pub fn pasta_conversa() -> PathBuf {
    pasta_dia().join("conversa")
}
fn mes_de(t: &SysTime) -> String {
    format!("{:04}-{:02}", t.ano, t.mes)
}
/// Arquivo de dados do mês (lido pelo app).
pub fn arquivo_conversa(t: &SysTime) -> PathBuf {
    pasta_conversa().join(format!("{}.jsonl", mes_de(t)))
}
/// Cópia legível do mês (para o usuário ler; a IA não lê).
pub fn arquivo_legivel(t: &SysTime) -> PathBuf {
    pasta_conversa().join(format!("{}.md", mes_de(t)))
}
/// Memória compactada (escrita pelo Haiku) e o ponto até onde ela já resumiu.
pub fn arquivo_resumo() -> PathBuf {
    pasta_dia().join("memoria-longa.md")
}
fn arquivo_estado_resumo() -> PathBuf {
    pasta_dia().join("memoria-longa.estado.json")
}
pub fn arquivo_lembretes() -> PathBuf {
    pasta_dia().join("lembretes.json")
}
pub fn sistema_md() -> PathBuf {
    config().prompt.clone()
}
pub fn claude_exe() -> PathBuf {
    let home = std::env::var("USERPROFILE").unwrap_or_default();
    let nativo = Path::new(&home).join(r".local\bin\claude.exe");
    if nativo.exists() {
        return nativo;
    }
    let appdata = std::env::var("APPDATA").unwrap_or_default();
    Path::new(&appdata).join(r"npm\claude.cmd")
}
fn pasta_trabalho() -> PathBuf {
    // fora da pasta de trabalho, para não carregar o CLAUDE.md do projeto em toda chamada interna
    let base = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| std::env::temp_dir().to_string_lossy().into());
    let p = Path::new(&base).join("Claudinho");
    let _ = std::fs::create_dir_all(&p);
    p
}
pub fn exe_irmao(nome: &str) -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|p| p.join(nome)))
        .unwrap_or_else(|| PathBuf::from(nome))
}

// ---------- modelos ----------
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Modelo {
    Haiku,
    Sonnet,
    Opus,
}
impl Modelo {
    pub fn alias(self) -> &'static str {
        match self {
            Modelo::Haiku => "haiku",
            Modelo::Sonnet => "sonnet",
            Modelo::Opus => "opus",
        }
    }
    pub fn esforco(self) -> &'static str {
        match self {
            Modelo::Haiku => "low",
            _ => "medium",
        }
    }
    pub fn nome(self) -> &'static str {
        match self {
            Modelo::Haiku => "Haiku",
            Modelo::Sonnet => "Sonnet",
            Modelo::Opus => "Opus",
        }
    }
    pub fn de(s: &str) -> Modelo {
        match s {
            "Sonnet" => Modelo::Sonnet,
            "Opus" => Modelo::Opus,
            _ => Modelo::Haiku,
        }
    }
}

#[derive(Clone)]
pub struct Msg {
    pub data: String, // AAAA-MM-DD
    pub hora: String,
    pub papel: String, // eu | claude | sistema | alerta | lider
    pub modelo: String,
    pub texto: String,
}
impl Msg {
    fn de_json(v: &Value, data_padrao: &str) -> Msg {
        Msg {
            data: v["data"].as_str().unwrap_or(data_padrao).into(),
            hora: v["hora"].as_str().unwrap_or("").into(),
            papel: v["papel"].as_str().unwrap_or("").into(),
            modelo: v["modelo"].as_str().unwrap_or("").into(),
            texto: v["texto"].as_str().unwrap_or("").into(),
        }
    }
    fn json(&self) -> String {
        json!({"data": self.data, "hora": self.hora, "papel": self.papel, "modelo": self.modelo, "texto": self.texto}).to_string()
    }
    /// Conta para a IA (o resto é bastidor: troca de líder, avisos do sistema).
    pub fn util(&self) -> bool {
        matches!(self.papel.as_str(), "eu" | "claude" | "alerta")
    }
    /// "21/11 09:12"
    pub fn quando(&self) -> String {
        let d: Vec<&str> = self.data.split('-').collect();
        if d.len() == 3 { format!("{}/{} {}", d[2], d[1], self.hora) } else { self.hora.clone() }
    }
}

/// Meses existentes (AAAA-MM), em ordem.
fn meses() -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(pasta_conversa())
        .map(|it| {
            it.filter_map(|e| e.ok())
                .filter_map(|e| e.file_name().to_str().and_then(|n| n.strip_suffix(".jsonl")).map(String::from))
                .filter(|n| n.len() == 7)
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}
fn ler_mes(mes: &str) -> Vec<Msg> {
    let Ok(f) = std::fs::File::open(pasta_conversa().join(format!("{mes}.jsonl"))) else { return vec![] };
    std::io::BufReader::new(f)
        .lines()
        .map_while(Result::ok)
        .filter_map(|l| serde_json::from_str::<Value>(&l).ok())
        .map(|v| Msg::de_json(&v, ""))
        .collect()
}

/// Últimas `n` mensagens da conversa inteira (lê só os meses necessários, do fim para o começo).
pub fn carregar_ultimas(n: usize) -> (Vec<Msg>, bool) {
    let mut acc: Vec<Msg> = vec![];
    let ms = meses();
    let mut tem_mais = false;
    for (i, mes) in ms.iter().enumerate().rev() {
        let mut v = ler_mes(mes);
        v.extend(acc);
        acc = v;
        if acc.len() >= n {
            tem_mais = acc.len() > n || i > 0;
            break;
        }
    }
    let ini = acc.len().saturating_sub(n);
    (acc.split_off(ini), tem_mais)
}

pub fn gravar(papel: &str, modelo: Modelo, texto: &str) -> Msg {
    let t = agora();
    let m = Msg { data: data_iso(&t), hora: hhmm(&t), papel: papel.into(), modelo: modelo.nome().into(), texto: texto.into() };
    let _ = std::fs::create_dir_all(pasta_conversa());
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(arquivo_conversa(&t)) {
        let _ = writeln!(f, "{}", m.json());
    }
    escrever_legivel(&m, &t);
    m
}

/// Acrescenta a mensagem na cópia legível do mês, com um título por dia.
fn escrever_legivel(m: &Msg, t: &SysTime) {
    if m.papel == "lider" {
        return;
    }
    let caminho = arquivo_legivel(t);
    let titulo = format!("## {}, {:02}/{:02}/{}", DIAS[t.dia_semana as usize % 7], t.dia, t.mes, t.ano);
    let ja_tem = std::fs::read_to_string(&caminho).map(|s| s.contains(&titulo)).unwrap_or(false);
    let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&caminho) else { return };
    if !ja_tem {
        let _ = writeln!(f, "\n{titulo}\n");
    }
    let linha = match m.papel.as_str() {
        "limpar" => format!("_{} · conversa limpa (/clear)_", m.hora),
        "eu" => format!("**{} · {}:** {}", m.hora, nome(), m.texto),
        "alerta" => format!("**{} · Claude ({}) 🔔:** {}", m.hora, m.modelo, m.texto),
        "sistema" => format!("_{} · {}_", m.hora, m.texto),
        _ => format!("**{} · Claude ({}):** {}", m.hora, m.modelo, m.texto),
    };
    let _ = writeln!(f, "{}\n", linha.replace('\n', "  \n"));
}

/// Converte os arquivos antigos por dia (AAAA-MM-DD.jsonl) para a conversa mensal.
/// O arquivo é movido para memoria\dia\antigo antes de ser lido, então dois processos não duplicam.
pub fn migrar_conversas_antigas() {
    let Ok(it) = std::fs::read_dir(pasta_dia()) else { return };
    let mut antigos: Vec<String> = it
        .filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().to_str().map(String::from))
        .filter(|n| n.len() == 16 && n.ends_with(".jsonl") && n.as_bytes()[4] == b'-' && n.as_bytes()[7] == b'-')
        .collect();
    antigos.sort();
    let destino = pasta_dia().join("antigo");
    for nome in antigos {
        let _ = std::fs::create_dir_all(&destino);
        let novo = destino.join(&nome);
        if std::fs::rename(pasta_dia().join(&nome), &novo).is_err() {
            continue;
        }
        let data = &nome[..10];
        let Ok(s) = std::fs::read_to_string(&novo) else { continue };
        let _ = std::fs::create_dir_all(pasta_conversa());
        let mes = &data[..7];
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(pasta_conversa().join(format!("{mes}.jsonl"))) {
            for l in s.lines() {
                if let Ok(v) = serde_json::from_str::<Value>(l) {
                    let m = Msg::de_json(&v, data);
                    let _ = writeln!(f, "{}", m.json());
                    let (a, me, d) = (data[..4].parse().unwrap_or(0), data[5..7].parse().unwrap_or(0), data[8..10].parse().unwrap_or(0));
                    let t = SysTime { ano: a, mes: me, dia: d, dia_semana: dia_da_semana(a, me, d), ..Default::default() };
                    escrever_legivel(&m, &t);
                }
            }
        }
    }
}
fn dia_da_semana(a: u16, m: u16, d: u16) -> u16 {
    // Zeller simplificado (0 = domingo)
    let (mut y, mut mm) = (a as i32, m as i32);
    if mm < 3 {
        mm += 12;
        y -= 1;
    }
    let h = (d as i32 + 13 * (mm + 1) / 5 + y + y / 4 - y / 100 + y / 400) % 7; // 0 = sábado
    ((h + 6) % 7) as u16
}

// ---------- memória longa (resumo compactado) ----------
/// Mensagens úteis ainda não resumidas, com o ponto (mês, byte) logo depois de cada uma.
fn nao_resumidas() -> Vec<(Msg, String, u64)> {
    let est: Value = std::fs::read_to_string(arquivo_estado_resumo()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(json!({}));
    let mes0 = est["mes"].as_str().unwrap_or("").to_string();
    let off0 = est["byte"].as_u64().unwrap_or(0);
    let mut saida = vec![];
    for mes in meses().into_iter().filter(|m| *m >= mes0) {
        let Ok(mut f) = std::fs::File::open(pasta_conversa().join(format!("{mes}.jsonl"))) else { continue };
        let mut pos = if mes == mes0 { off0 } else { 0 };
        use std::io::Seek;
        if f.seek(std::io::SeekFrom::Start(pos)).is_err() {
            continue;
        }
        let mut r = std::io::BufReader::new(f);
        let mut linha = String::new();
        loop {
            linha.clear();
            let Ok(n) = r.read_line(&mut linha) else { break };
            if n == 0 {
                break;
            }
            pos += n as u64;
            if let Ok(v) = serde_json::from_str::<Value>(linha.trim()) {
                let m = Msg::de_json(&v, "");
                if m.util() || m.papel == "limpar" {
                    saida.push((m, mes.clone(), pos));
                }
            }
        }
    }
    saida
}

/// Quantas mensagens recentes ficam sempre inteiras no prompt.
pub const HIST_CURTO: usize = 14;
/// A partir de quantas mensagens não resumidas vale compactar.
pub const LIMITE_COMPACTAR: usize = 40;
/// Teto de mensagens enviadas por prompt (proteção se a compactação falhar).
const TETO_PROMPT: usize = 60;

pub fn precisa_compactar() -> bool {
    nao_resumidas().iter().filter(|(m, _, _)| m.util()).count() >= LIMITE_COMPACTAR
}

const SISTEMA_RESUMO: &str = "Você mantém a memória de longo prazo do Claudinho, o assistente pessoal de {NOME} que roda o dia inteiro. \
Recebe a MEMÓRIA ATUAL e MENSAGENS NOVAS da conversa e devolve a memória atualizada, em português, em markdown, com exatamente estas seções:\n\
## Sobre {NOME}\n(fatos estáveis, preferências, rotina, objetivos)\n\
## Combinados e regras\n(o que ele pediu para o assistente fazer ou evitar)\n\
## Em aberto\n(pendências, prazos, decisões a acompanhar; tire o que foi resolvido)\n\
## Diário\n(uma linha por dia, a mais recente por último: `AAAA-MM-DD — o que importou`; dias com mais de 30 dias viram uma linha por mês)\n\n\
Regras: no máximo 4000 caracteres no total; descarte conversa fiada e repetição; nunca invente nada que não esteja nas mensagens ou na memória; \
não registre senhas, códigos, números de documentos, cartões ou valores de conta; não registre conteúdo sensível sobre saúde além do necessário para ajudar. \
Responda só com o markdown, sem comentários.";

/// Dobra as mensagens antigas no resumo (roda em segundo plano, pelo vigia). Devolve true se resumiu.
pub fn compactar() -> Result<bool, String> {
    let todas: Vec<(Msg, String, u64)> = nao_resumidas().into_iter().filter(|(m, _, _)| m.util()).collect();
    if todas.len() < LIMITE_COMPACTAR {
        return Ok(false);
    }
    let ate = (todas.len() - HIST_CURTO).min(150);
    let lote = &todas[..ate];
    let atual = std::fs::read_to_string(arquivo_resumo()).unwrap_or_else(|_| "(vazia)".into());
    let msgs: Vec<String> = lote
        .iter()
        .map(|(m, _, _)| {
            let quem = if m.papel == "eu" { nome().to_string() } else { format!("Claude ({})", m.modelo) };
            format!("[{}] {}: {}", m.quando(), quem, m.texto.chars().take(1200).collect::<String>())
        })
        .collect();
    let prompt = format!("# MEMÓRIA ATUAL\n{}\n\n# MENSAGENS NOVAS\n{}\n", atual.trim(), msgs.join("\n"));
    let novo = chamar_claude_com(Modelo::Haiku, Sistema::Texto(SISTEMA_RESUMO.replace("{NOME}", nome())), &[], &prompt, None)?;
    let novo = novo.trim();
    if novo.len() < 40 || !novo.contains("##") {
        return Err("resumo veio vazio ou fora do formato".into());
    }
    let novo: String = novo.chars().take(8000).collect();
    let tmp = arquivo_resumo().with_extension("md.tmp");
    std::fs::write(&tmp, &novo).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, arquivo_resumo()).map_err(|e| e.to_string())?;
    let (_, mes, byte) = &lote[lote.len() - 1];
    let _ = std::fs::write(arquivo_estado_resumo(), json!({"mes": mes, "byte": byte}).to_string());
    Ok(true)
}
pub fn lider_atual(conv: &[Msg]) -> Modelo {
    conv.iter().rev().find(|m| m.papel == "lider").map(|m| Modelo::de(&m.modelo)).unwrap_or(Modelo::Haiku)
}

// ---------- lembretes ----------
pub fn ler_lembretes() -> Vec<Value> {
    std::fs::read_to_string(arquivo_lembretes())
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default()
}
pub fn salvar_lembretes(v: &[Value]) {
    let _ = std::fs::create_dir_all(pasta_dia());
    let _ = std::fs::write(arquivo_lembretes(), serde_json::to_string_pretty(&Value::Array(v.to_vec())).unwrap_or_default());
}
/// Extrai [[LEMBRETE ...]], grava em lembretes.json e devolve o texto limpo.
pub fn tirar_lembretes(texto: &str) -> String {
    let t = agora();
    let mut saida = vec![];
    let mut novos = vec![];
    for linha in texto.lines() {
        let l = linha.trim();
        if let Some(resto) = l.strip_prefix("[[LEMBRETE").and_then(|r| r.strip_suffix("]]")) {
            let (quando, txt) = resto.split_once('|').unwrap_or((resto, ""));
            let partes: Vec<&str> = quando.split_whitespace().collect();
            let (data, hora) = match partes.as_slice() {
                [d, h] => {
                    let (dd, mm) = d.split_once('/').unwrap_or(("", ""));
                    (format!("{:04}-{:0>2}-{:0>2}", t.ano, mm, dd), h.to_string())
                }
                [h] => (data_iso(&t), h.to_string()),
                _ => continue,
            };
            let hora = if hora.len() == 4 { format!("0{hora}") } else { hora };
            novos.push(json!({"data": data, "hora": hora, "texto": txt.trim(), "feito": false}));
        } else {
            saida.push(linha);
        }
    }
    if !novos.is_empty() {
        let mut v = ler_lembretes();
        v.extend(novos);
        salvar_lembretes(&v);
    }
    saida.join("\n").trim().to_string()
}
/// Marca e devolve os lembretes vencidos.
pub fn lembretes_vencidos() -> Vec<String> {
    let t = agora();
    let chave_agora = format!("{} {}", data_iso(&t), hhmm(&t));
    let mut v = ler_lembretes();
    let mut saiu = vec![];
    for l in v.iter_mut() {
        if l["feito"].as_bool().unwrap_or(false) {
            continue;
        }
        let chave = format!("{} {}", l["data"].as_str().unwrap_or(""), l["hora"].as_str().unwrap_or(""));
        if chave <= chave_agora {
            l["feito"] = json!(true);
            saiu.push(l["texto"].as_str().unwrap_or("").to_string());
        }
    }
    if !saiu.is_empty() {
        salvar_lembretes(&v);
    }
    saiu
}

// ---------- IA ----------
/// "[sábado 21/11 10:56]" — vai na frente de cada mensagem da sessão contínua.
pub fn carimbo() -> String {
    let t = agora();
    format!("[{} {:02}/{:02} {}]", DIAS[t.dia_semana as usize % 7], t.dia, t.mes, hhmm(&t))
}

/// Contexto mandado no começo de uma sessão nova do Claude Code (a primeira vez, depois de
/// um /clear ou se a sessão antiga se perder): memória longa, agenda, lembretes e o fim da conversa.
pub fn contexto_inicio_sessao() -> String {
    let agenda = std::fs::read_to_string(pasta_dia().join("agenda.md")).unwrap_or_else(|_| "(sem agenda)".into());
    let memoria = std::fs::read_to_string(arquivo_resumo()).unwrap_or_else(|_| "(ainda vazia)".into());
    let pend: Vec<String> = ler_lembretes()
        .iter()
        .filter(|l| !l["feito"].as_bool().unwrap_or(false))
        .map(|l| format!("- {} {} — {}", l["data"].as_str().unwrap_or(""), l["hora"].as_str().unwrap_or(""), l["texto"].as_str().unwrap_or("")))
        .collect();
    let mut recentes: Vec<Msg> = nao_resumidas().into_iter().map(|(m, _, _)| m).collect();
    if let Some(i) = recentes.iter().rposition(|m| m.papel == "limpar") {
        recentes.drain(..=i);
    }
    let ini = recentes.len().saturating_sub(12);
    let hist: Vec<String> = recentes[ini..]
        .iter()
        .map(|m| {
            let quem = if m.papel == "eu" { nome().to_string() } else { format!("Claude ({})", m.modelo) };
            format!("[{}] {}: {}", m.quando(), quem, m.texto.chars().take(600).collect::<String>())
        })
        .collect();
    format!(
        "<contexto-claudinho>\nInício de uma sessão nova do Claudinho. Use isto como memória; não responda a este bloco.\n\n## Memória longa\n{}\n\n## Agenda\n{}\n\n## Lembretes pendentes\n{}\n\n## Últimas mensagens antes desta sessão\n{}\n</contexto-claudinho>\n\n",
        memoria.trim(),
        agenda.trim(),
        if pend.is_empty() { "(nenhum)".into() } else { pend.join("\n") },
        if hist.is_empty() { "(nenhuma)".into() } else { hist.join("\n") },
    )
}

const SISTEMA_ASSUNTO: &str = "Você decide se uma conversa de chat deve continuar ou recomeçar. Recebe as últimas mensagens e uma mensagem nova de {NOME}. Não responda à mensagem: só classifique.
NOVO = a mensagem nova fala de outra coisa e dá para responder sem nada da conversa anterior (outro tema, outra pergunta solta).
MESMO = ela continua, corrige ou detalha o tema anterior, ou depende dele (cita \"isso\", \"e o outro\", \"perdão\", \"e os próximos\", o arquivo/tarefa de antes).
Exemplos:
- antes: vagas de emprego | nova: \"a Ana Maria Braga é casada com quem\" -> NOVO
- antes: jogos do Flu | nova: \"quais processadores são melhores pro meu PC\" -> NOVO
- antes: peças de PC | nova: \"vai ter temporal hoje aqui?\" -> NOVO
- antes: últimos jogos do Flu | nova: \"perdão, dos 3 próximos\" -> MESMO
- antes: currículo | nova: \"muda a cor do título\" -> MESMO
Responda só uma palavra: NOVO ou MESMO.";

/// Pergunta rápida ao Sonnet (sem ferramentas nem conectores, ~2 mil tokens): a mensagem nova muda
/// de assunto? Testado: Sonnet 5/5, Haiku 1/5 (respondia com frases ou à própria pergunta).
/// Erro ou resposta estranha = mesmo assunto (não limpa nada).
pub fn assunto_novo(anteriores: &str, nova: &str) -> bool {
    let prompt = format!("## Fim da conversa\n{anteriores}\n\n## Mensagem nova\n{nova}");
    let r = match chamar_claude_com(Modelo::Sonnet, Sistema::Texto(SISTEMA_ASSUNTO.replace("{NOME}", nome())), &[], &prompt, None) {
        Ok(r) => r.to_uppercase(),
        Err(_) => return false,
    };
    // a primeira das duas palavras que aparecer decide (tolera "**NOVO**" ou uma frase curta)
    match (r.find("NOVO"), r.find("MESMO")) {
        (Some(n), Some(m)) => n < m,
        (Some(_), None) => true,
        _ => false,
    }
}

/// Prompt enxuto: memória longa compactada + só as mensagens ainda não resumidas (no máximo 60).
/// O tamanho fica estável para sempre, por maior que a conversa fique.
pub fn montar_prompt(nova: &str, nota: Option<&str>) -> String {
    let t = agora();
    let agenda = std::fs::read_to_string(pasta_dia().join("agenda.md")).unwrap_or_else(|_| "(sem agenda)".into());
    let pend: Vec<String> = ler_lembretes()
        .iter()
        .filter(|l| !l["feito"].as_bool().unwrap_or(false))
        .map(|l| format!("- {} {} — {}", l["data"].as_str().unwrap_or(""), l["hora"].as_str().unwrap_or(""), l["texto"].as_str().unwrap_or("")))
        .collect();
    let mut uteis: Vec<Msg> = nao_resumidas().into_iter().map(|(m, _, _)| m).collect();
    // /clear: a conversa recente começa depois da última limpeza (a memória longa continua)
    if let Some(i) = uteis.iter().rposition(|m| m.papel == "limpar") {
        uteis.drain(..=i);
    }
    // a mensagem nova já foi gravada; não repete
    if let Some(u) = uteis.last() {
        if u.papel == "eu" && nova.ends_with(&u.texto) {
            uteis.pop();
        }
    }
    let inicio = uteis.len().saturating_sub(TETO_PROMPT);
    let n = uteis.len() - inicio;
    let hist: Vec<String> = uteis[inicio..]
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let quem = match m.papel.as_str() {
                "eu" => nome().to_string(),
                "alerta" => format!("Claude ({}) [alerta]", m.modelo),
                _ => format!("Claude ({})", m.modelo),
            };
            // as recentes inteiras; as mais velhas encurtadas
            let lim = if i + HIST_CURTO >= n { 1500 } else { 300 };
            let txt: String = m.texto.chars().take(lim).collect();
            format!("[{}] {}: {}", m.quando(), quem, txt)
        })
        .collect();
    let memoria = std::fs::read_to_string(arquivo_resumo()).unwrap_or_else(|_| "(ainda vazia)".into());
    let mut p = format!(
        "Agora: {}, {:02}/{:02}/{} {}.\n\n## Agenda\n{}\n\n## Lembretes pendentes\n{}\n\n## Memória longa (resumo de tudo que veio antes)\n{}\n\n## Conversa recente (mais antigas primeiro)\n{}\n\n",
        DIAS[t.dia_semana as usize % 7],
        t.dia,
        t.mes,
        t.ano,
        hhmm(&t),
        agenda.trim(),
        if pend.is_empty() { "(nenhum)".into() } else { pend.join("\n") },
        memoria.trim(),
        if hist.is_empty() { "(ainda vazia)".into() } else { hist.join("\n") },
    );
    if let Some(n) = nota {
        p.push_str(&format!("## Nota interna\n{n}\n\n"));
    }
    p.push_str(&format!("## Nova mensagem\n{nova}\n"));
    p
}

/// Mesmas permissões de uma conversa do Claude Code do usuário: todas as ferramentas padrão
/// (PowerShell, Bash, Edit, Write, Web, conectores) no modo "auto". O que o modo auto
/// liberar roda direto; o que pedir aprovação vira um cartão Permitir/Negar no chat.
pub fn chamar_claude(modelo: Modelo, prompt: &str, avisos: &dyn Fn(Resultado)) -> Result<String, String> {
    chamar_claude_com(modelo, Sistema::Assistente, &["default"], prompt, Some(avisos))
}

/// Texto curto do que a ferramenta quer fazer, para o cartão de aprovação.
fn resumo_ferramenta(nome: &str, input: &Value) -> String {
    let s = |k: &str| input[k].as_str().unwrap_or("").to_string();
    let r = match nome {
        "Bash" | "PowerShell" => {
            let d = s("description");
            if d.is_empty() { s("command") } else { format!("{d}\n{}", s("command")) }
        }
        "Edit" | "Write" | "Read" | "NotebookEdit" => s("file_path"),
        "WebFetch" => s("url"),
        "WebSearch" => s("query"),
        _ => input.to_string(),
    };
    r.chars().take(600).collect()
}

pub enum Sistema {
    /// o prompt do Claudinho (dia\sistema.md) + o papel do modelo
    Assistente,
    /// um prompt de sistema próprio (ex.: compactar a memória)
    Texto(String),
}

/// Roda o Claude Code em modo stream-json: manda o prompt, mostra o progresso e
/// responde os pedidos de permissão pelo app (`avisos` None = nega tudo que pedir aprovação).
pub fn chamar_claude_com(modelo: Modelo, sistema: Sistema, ferramentas: &[&str], prompt: &str, avisos: Option<&dyn Fn(Resultado)>) -> Result<String, String> {
    use std::os::windows::process::CommandExt;
    let mut cmd = Command::new(claude_exe());
    cmd.current_dir(pasta_trabalho())
        .args(["-p", "--model", modelo.alias(), "--effort", modelo.esforco()])
        .args(["--no-session-persistence", "--verbose", "--input-format", "stream-json", "--output-format", "stream-json"]);
    match sistema {
        Sistema::Assistente => {
            let papel = if modelo == Modelo::Haiku {
                "Você está rodando como Claude Haiku, o vigia do dia.".to_string()
            } else {
                format!("Você está rodando como Claude {}, chamado para assumir a conversa. Quando o assunto pesado terminar, comece com [[HAIKU]].", modelo.nome())
            };
            cmd.arg("--system-prompt-file").arg(sistema_md()).args(["--append-system-prompt", &papel]).arg("--add-dir").arg(pasta_projetos());
        }
        Sistema::Texto(ref s) => {
            cmd.args(["--system-prompt", s.as_str()]);
        }
    }
    cmd.args(["--tools", &ferramentas.join(",")]);
    let modo = if ferramentas == ["default"] { "auto" } else { "dontAsk" };
    // chamada interna (resumo, verificação, assunto): sem conectores. Com eles o CLI carrega os
    // esquemas de todos os servidores MCP do claude.ai inteiros: ~216 mil tokens a cada chamada (medido).
    if modo != "auto" {
        cmd.arg("--strict-mcp-config");
    }
    cmd.args(["--permission-mode", modo]);
    if avisos.is_some() && modo == "auto" {
        cmd.args(["--permission-prompts", "host", "--permission-prompt-tool", "stdio"]);
    } else {
        cmd.args(["--permission-prompts", "none"]);
    }
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    let mut filho = cmd.spawn().map_err(|e| format!("não consegui abrir o Claude CLI ({}): {e}", claude_exe().display()))?;
    let mut entrada = filho.stdin.take();
    let mandar = |entrada: &mut Option<std::process::ChildStdin>, v: Value| {
        if let Some(si) = entrada.as_mut() {
            let _ = writeln!(si, "{v}");
            let _ = si.flush();
        }
    };
    // stdout linha a linha numa thread; stderr guardado para mensagens de erro
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    if let Some(out) = filho.stdout.take() {
        std::thread::spawn(move || {
            for l in std::io::BufReader::new(out).lines().map_while(Result::ok) {
                if tx.send(l).is_err() {
                    break;
                }
            }
        });
    }
    let erros = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
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
    mandar(&mut entrada, json!({"type": "control_request", "request_id": "ini", "request": {"subtype": "initialize"}}));
    mandar(&mut entrada, json!({"type": "user", "session_id": "", "parent_tool_use_id": null, "message": {"role": "user", "content": prompt}}));

    let mut prazo = Instant::now() + Duration::from_secs(15 * 60);
    let resultado: Result<String, String> = loop {
        if Instant::now() > prazo {
            break Err("demorou demais (15 min) e cancelei".into());
        }
        let linha = match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(l) => l,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(_) => {
                let e = erros.lock().unwrap().clone();
                break Err(format!("o Claude CLI parou sem responder: {}", e.chars().take(300).collect::<String>()));
            }
        };
        let Ok(v) = serde_json::from_str::<Value>(&linha) else { continue };
        match v["type"].as_str().unwrap_or("") {
            "control_request" => {
                let id = v["request_id"].clone();
                let req = &v["request"];
                if req["subtype"] == "can_use_tool" {
                    let nome = req["tool_name"].as_str().unwrap_or("?").to_string();
                    let input = req["input"].clone();
                    let mut ok = false;
                    if let Some(av) = avisos {
                        let (rtx, rrx) = std::sync::mpsc::channel::<bool>();
                        av(Resultado::Permissao { ferramenta: nome.clone(), resumo: resumo_ferramenta(&nome, &input), resposta: rtx });
                        let espera = Instant::now();
                        ok = rrx.recv_timeout(Duration::from_secs(10 * 60)).unwrap_or(false);
                        prazo += espera.elapsed(); // o tempo esperando o usuário não conta
                    }
                    let resp = if ok {
                        json!({"behavior": "allow", "updatedInput": input})
                    } else {
                        json!({"behavior": "deny", "message": format!("{} não aprovou essa ação (ou não respondeu). Não tente contornar; diga o que ficou faltando.", crate::comum::nome())})
                    };
                    mandar(&mut entrada, json!({"type": "control_response", "response": {"subtype": "success", "request_id": id, "response": resp}}));
                } else {
                    mandar(&mut entrada, json!({"type": "control_response", "response": {"subtype": "error", "request_id": id, "error": "não suportado pelo Claudinho"}}));
                }
            }
            "assistant" => {
                if let (Some(av), Some(itens)) = (avisos, v["message"]["content"].as_array()) {
                    for it in itens.iter().filter(|i| i["type"] == "tool_use") {
                        let nome = it["name"].as_str().unwrap_or("ferramenta");
                        let d = it["input"]["description"].as_str().unwrap_or("");
                        av(Resultado::Progresso(if d.is_empty() { format!("usando {nome}…") } else { format!("{d}…") }));
                    }
                }
            }
            "result" => {
                let r = v["result"].as_str().unwrap_or("").to_string();
                if v["is_error"].as_bool().unwrap_or(false) {
                    if r.contains("authenticate") || r.contains("OAuth") || r.contains("log in") {
                        break Err("o Claude CLI está deslogado. Rode o instalar-e-logar.ps1 (pasta claude-popup\\dia)".into());
                    }
                    break Err(if r.is_empty() { v["subtype"].as_str().unwrap_or("erro").to_string() } else { r });
                }
                break Ok(r);
            }
            _ => {}
        }
    };
    drop(entrada); // fecha a entrada: o CLI encerra
    let fim = Instant::now();
    while fim.elapsed() < Duration::from_secs(5) {
        if let Ok(Some(_)) = filho.try_wait() {
            return resultado;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = filho.kill();
    resultado
}

pub enum Pedido {
    Mensagem(String),
    Verificacao(String),
}
pub enum Resultado {
    Sistema(String),
    Resposta { modelo: Modelo, texto: String, alerta: bool },
    Lider(Modelo),
    Erro(String),
    /// o que o Claude está fazendo agora (ex.: "rodando PowerShell…")
    Progresso(String),
    /// pedido de aprovação: o app mostra Permitir/Negar e responde pelo canal
    Permissao { ferramenta: String, resumo: String, resposta: std::sync::mpsc::Sender<bool> },
    Fim,
}

/// Roda um pedido com o líder atual, cuidando da escalada Haiku → Sonnet → Opus e da devolução.
pub fn trabalhar(pedido: Pedido, mut lider: Modelo, enviar: impl Fn(Resultado)) {
    let (nova, alerta) = match &pedido {
        Pedido::Mensagem(s) => (format!("[{}] {}: {s}", hhmm(&agora()), nome()), false),
        Pedido::Verificacao(h) => (format!("[VERIFICAÇÃO {h}]"), true),
    };
    if alerta {
        lider = Modelo::Haiku;
    }
    let mut nota: Option<String> = None;
    for _ in 0..3 {
        let prompt = montar_prompt(&nova, nota.as_deref());
        // verificação automática (ninguém pediu nada): só leitura, garantido no código
        let r = if alerta { chamar_claude_com(lider, Sistema::Assistente, &["Read", "Glob", "Grep"], &prompt, None) } else { chamar_claude(lider, &prompt, &enviar) };
        match r {
            Err(e) => {
                enviar(Resultado::Erro(e));
                break;
            }
            Ok(r) => {
                let r = r.trim().to_string();
                let primeira = r.lines().next().unwrap_or("").trim().to_string();
                let escala = if primeira.starts_with("[[OPUS]]") && lider != Modelo::Opus && !alerta {
                    Some(Modelo::Opus)
                } else if primeira.starts_with("[[SONNET]]") && lider == Modelo::Haiku && !alerta {
                    Some(Modelo::Sonnet)
                } else {
                    None
                };
                if let Some(novo) = escala {
                    let motivo = primeira.trim_start_matches("[[OPUS]]").trim_start_matches("[[SONNET]]").trim().to_string();
                    enviar(Resultado::Sistema(format!(
                        "{} chamou o {}{}",
                        lider.nome(),
                        novo.nome(),
                        if motivo.is_empty() { String::new() } else { format!(": {motivo}") }
                    )));
                    nota = Some(format!("O {} te chamou para assumir: {motivo}. Responda você a nova mensagem.", lider.nome()));
                    lider = novo;
                    enviar(Resultado::Lider(lider));
                    continue;
                }
                if r.contains("[[SILENCIO]]") {
                    break;
                }
                if primeira.starts_with("[[HAIKU]]") {
                    let texto = r.replacen("[[HAIKU]]", "", 1);
                    enviar(Resultado::Resposta { modelo: lider, texto: tirar_lembretes(texto.trim()), alerta });
                    enviar(Resultado::Lider(Modelo::Haiku));
                    break;
                }
                enviar(Resultado::Resposta { modelo: lider, texto: tirar_lembretes(&r), alerta });
                break;
            }
        }
    }
    enviar(Resultado::Fim);
}

// ---------- mascote: balãozinho de conversa sorrindo ----------
/// Cor principal do Claudinho (verde-azulado).
pub const COR_MASCOTE: u32 = 0x1E9C8C;
fn mistura(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}
fn rgb(h: u32) -> [f32; 3] {
    [((h >> 16) & 255) as f32, ((h >> 8) & 255) as f32, (h & 255) as f32]
}
/// O ponto está no rabinho do balão (triângulo embaixo, à esquerda)?
fn no_rabinho(u: f32, v: f32) -> bool {
    let (a, b, c) = ((-0.62f32, 0.50f32), (-0.92f32, 0.93f32), (-0.22f32, 0.80f32));
    let s = |p: (f32, f32), q: (f32, f32)| (u - q.0) * (p.1 - q.1) - (p.0 - q.0) * (v - q.1);
    let (d1, d2, d3) = (s(a, b), s(b, c), s(c, a));
    !((d1 < 0.0 || d2 < 0.0 || d3 < 0.0) && (d1 > 0.0 || d2 > 0.0 || d3 > 0.0))
}
/// Cor (rgb, alfa) num ponto normalizado (raio ~0.86 = borda do corpo).
fn amostra_mascote(u: f32, v: f32) -> Option<([f32; 3], f32)> {
    let r = (u * u + v * v).sqrt();
    let borda = 0.86;
    let rabo = no_rabinho(u, v);
    if r > borda && !rabo {
        return None;
    }
    let base = rgb(COR_MASCOTE);
    // luz de cima à esquerda, aro um pouco mais escuro
    let mut c = mistura(base, rgb(0x5CCBBB), ((-u - v) * 0.30 + 0.10).clamp(0.0, 0.40));
    if r > borda - 0.07 && !rabo {
        c = mistura(c, rgb(0x147568), 0.5);
    }
    if rabo && r > borda {
        c = mistura(base, rgb(0x147568), 0.25);
    }
    // bochechas
    for sx in [-1.0f32, 1.0] {
        let (du, dv) = (u - sx * 0.50, v - 0.20);
        if du * du + dv * dv < 0.11 * 0.11 {
            c = mistura(c, rgb(0xF7B7A3), 0.65);
        }
    }
    let marrom = rgb(0x183B36);
    // olhos
    for sx in [-1.0f32, 1.0] {
        let (du, dv) = ((u - sx * 0.27) / 0.09, (v + 0.10) / 0.14);
        if du * du + dv * dv <= 1.0 {
            let (hu, hv) = (u - sx * 0.27 - 0.03, v + 0.15);
            if hu * hu + hv * hv < 0.035 * 0.035 {
                return Some((rgb(0xFFF8EE), 1.0));
            }
            return Some((marrom, 1.0));
        }
    }
    // sorriso
    let d = (u * u + (v + 0.02) * (v + 0.02)).sqrt();
    if (d - 0.36).abs() < 0.05 && v > 0.13 && u.abs() < 0.34 {
        return Some((marrom, 1.0));
    }
    Some((c, 1.0))
}
/// Mascote em RGBA (alfa não pré-multiplicado), n×n, com supersampling 4×4.
pub fn mascote_rgba(n: u32) -> Vec<u8> {
    let mut px = vec![0u8; (n * n * 4) as usize];
    let c = n as f32 / 2.0;
    let raio = n as f32 * 0.485 / 0.965;
    for y in 0..n {
        for x in 0..n {
            let (mut sr, mut sg, mut sb, mut sa) = (0.0, 0.0, 0.0, 0.0);
            for sy in 0..4 {
                for sx in 0..4 {
                    let u = (x as f32 + (sx as f32 + 0.5) / 4.0 - c) / raio;
                    let v = (y as f32 + (sy as f32 + 0.5) / 4.0 - c) / raio;
                    if let Some((cor, a)) = amostra_mascote(u, v) {
                        sr += cor[0] * a;
                        sg += cor[1] * a;
                        sb += cor[2] * a;
                        sa += a;
                    }
                }
            }
            let i = ((y * n + x) * 4) as usize;
            if sa > 0.0 {
                px[i] = (sr / sa) as u8;
                px[i + 1] = (sg / sa) as u8;
                px[i + 2] = (sb / sa) as u8;
                px[i + 3] = (sa / 16.0 * 255.0) as u8;
            }
        }
    }
    px
}
/// Grava um .ico (BMP 32 bits) com vários tamanhos.
pub fn salvar_ico(caminho: &Path, tamanhos: &[u32]) -> std::io::Result<()> {
    let mut imagens = vec![];
    for &n in tamanhos {
        let rgba = mascote_rgba(n);
        let mut b = vec![];
        let hdr: [u32; 10] = [40, n, n * 2, 1 | (32 << 16), 0, 0, 0, 0, 0, 0];
        for h in hdr {
            b.extend_from_slice(&h.to_le_bytes());
        }
        for y in (0..n).rev() {
            for x in 0..n {
                let i = ((y * n + x) * 4) as usize;
                b.extend_from_slice(&[rgba[i + 2], rgba[i + 1], rgba[i], rgba[i + 3]]);
            }
        }
        let linha = ((n + 31) / 32) * 4;
        b.extend(std::iter::repeat_n(0u8, (linha * n) as usize));
        imagens.push((n, b));
    }
    let mut f = vec![0u8, 0, 1, 0];
    f.extend_from_slice(&(imagens.len() as u16).to_le_bytes());
    let mut deslocamento = 6 + 16 * imagens.len() as u32;
    for (n, b) in &imagens {
        let lado = if *n >= 256 { 0 } else { *n as u8 };
        f.extend_from_slice(&[lado, lado, 0, 0, 1, 0, 32, 0]);
        f.extend_from_slice(&(b.len() as u32).to_le_bytes());
        f.extend_from_slice(&deslocamento.to_le_bytes());
        deslocamento += b.len() as u32;
    }
    for (_, b) in &imagens {
        f.extend_from_slice(b);
    }
    std::fs::write(caminho, f)
}
pub fn caminho_ico() -> PathBuf {
    pasta_trabalho().join("claudinho.ico")
}
