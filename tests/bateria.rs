//! Bateria de testes do Claudinho.
//!
//! Rápidos (sem rede):          cargo test --release
//! Com o Claude Code de verdade: cargo test --release -- --ignored --test-threads=1
//!
//! Tudo roda numa pasta temporária própria (CLAUDINHO_CONFIG), sem tocar na conversa,
//! na memória ou nos lembretes reais.

#![allow(dead_code)]
#[path = "../src/dia_comum.rs"]
mod comum;
#[path = "../src/sessao.rs"]
mod sessao;

use comum::*;
use serde_json::json;
use sessao::{Evento, Sessao};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex, MutexGuard, Once};
use std::time::{Duration, Instant};

// ---------- ambiente isolado ----------
static INICIO: Once = Once::new();
static TRAVA: Mutex<()> = Mutex::new(());

fn base_teste() -> PathBuf {
    PathBuf::from(std::env::var("TEMP").unwrap()).join(format!("cl-teste-{}", std::process::id()))
}

/// Cria o config de teste (uma vez) e serializa os testes que mexem em arquivos.
fn preparar() -> MutexGuard<'static, ()> {
    INICIO.call_once(|| {
        let b = base_teste();
        let _ = std::fs::remove_dir_all(&b);
        std::fs::create_dir_all(b.join("trabalho")).unwrap();
        std::fs::create_dir_all(b.join("dados")).unwrap();
        std::fs::write(b.join("prompt.md"), include_str!("../prompt/prompt-modelo.md")).unwrap();
        let cfg = json!({
            "nome": "Teste",
            "pasta_trabalho": b.join("trabalho"),
            "pasta_dados": b.join("dados"),
            "prompt": b.join("prompt.md"),
        });
        std::fs::write(b.join("config.json"), cfg.to_string()).unwrap();
        unsafe { std::env::set_var("CLAUDINHO_CONFIG", b.join("config.json")) };
        assert_eq!(pasta_dia(), b.join("dados"), "config de teste não foi usado");
    });
    TRAVA.lock().unwrap_or_else(|e| e.into_inner())
}

fn limpar_lembretes() {
    let _ = std::fs::remove_file(arquivo_lembretes());
}

// ---------- lembretes ----------
#[test]
fn lembrete_de_hoje_sai_do_texto_e_vai_para_o_arquivo() {
    let _g = preparar();
    limpar_lembretes();
    let r = tirar_lembretes("Combinado!\n[[LEMBRETE 9:05 | ligar pro banco]]\nAté já.");
    assert_eq!(r, "Combinado!\nAté já.");
    let v = ler_lembretes();
    assert_eq!(v.len(), 1);
    assert_eq!(v[0]["hora"], "09:05", "hora com 1 dígito precisa de zero na frente");
    assert_eq!(v[0]["data"], data_iso(&agora()));
    assert_eq!(v[0]["texto"], "ligar pro banco");
    assert_eq!(v[0]["feito"], false);
}

#[test]
fn lembrete_de_outro_dia_usa_data_completa() {
    let _g = preparar();
    limpar_lembretes();
    tirar_lembretes("[[LEMBRETE 5/1 14:30 | dentista]]");
    let v = ler_lembretes();
    assert_eq!(v.len(), 1);
    assert_eq!(v[0]["data"], format!("{:04}-01-05", agora().ano));
    assert_eq!(v[0]["hora"], "14:30");
}

#[test]
fn lembrete_mal_formado_nao_quebra_nem_grava() {
    let _g = preparar();
    limpar_lembretes();
    let r = tirar_lembretes("[[LEMBRETE]]\ntexto normal com [[LEMBRETE no meio\n[[LEMBRETE a b c | x]]");
    assert!(r.contains("texto normal"));
    assert!(ler_lembretes().is_empty());
}

#[test]
fn lembrete_vencido_dispara_uma_vez_so() {
    let _g = preparar();
    limpar_lembretes();
    salvar_lembretes(&[
        json!({"data": "2000-01-01", "hora": "08:00", "texto": "velho", "feito": false}),
        json!({"data": "2999-01-01", "hora": "08:00", "texto": "futuro", "feito": false}),
        json!({"data": "2000-01-01", "hora": "08:00", "texto": "já feito", "feito": true}),
    ]);
    assert_eq!(lembretes_vencidos(), vec!["velho".to_string()]);
    assert!(lembretes_vencidos().is_empty(), "disparou duas vezes");
}

// ---------- conversa ----------
#[test]
fn gravar_e_carregar_ultimas() {
    let _g = preparar();
    for i in 0..5 {
        gravar("eu", Modelo::Sonnet, &format!("msg {i}"));
    }
    let (v, _) = carregar_ultimas(3);
    assert_eq!(v.len(), 3);
    assert_eq!(v.last().unwrap().texto, "msg 4");
    assert!(arquivo_legivel(&agora()).exists(), "cópia legível .md não foi escrita");
}

#[test]
fn lider_e_modelos() {
    let m = |papel: &str, modelo: &str| Msg { data: "2026-11-21".into(), hora: "10:00".into(), papel: papel.into(), modelo: modelo.into(), texto: String::new() };
    assert_eq!(lider_atual(&[m("lider", "Opus"), m("eu", "Opus")]), Modelo::Opus);
    assert_eq!(lider_atual(&[m("lider", "Opus"), m("lider", "Sonnet")]), Modelo::Sonnet);
    for x in [Modelo::Haiku, Modelo::Sonnet, Modelo::Opus] {
        assert_eq!(Modelo::de(x.nome()), x);
    }
    assert_eq!(m("eu", "Sonnet").quando(), "21/11 10:00");
    assert!(!m("sistema", "Sonnet").util());
}

// ---------- sessão salva ----------
#[test]
fn id_salvo_e_esquecido() {
    let _g = preparar();
    sessao::salvar_id("abc");
    sessao::salvar_contexto(1234);
    assert_eq!(sessao::id_salvo().as_deref(), Some("abc"));
    assert_eq!(sessao::contexto_salvo(), 1234);
    sessao::esquecer_id();
    assert_eq!(sessao::id_salvo(), None);
    assert_eq!(sessao::contexto_salvo(), 0);
}

#[test]
fn uuid_tem_formato_v4_e_nao_repete() {
    let a = sessao::uuid4();
    let b = sessao::uuid4();
    assert_ne!(a, b);
    let p: Vec<&str> = a.split('-').collect();
    assert_eq!(p.iter().map(|s| s.len()).collect::<Vec<_>>(), vec![8, 4, 4, 4, 12]);
    assert!(p[2].starts_with('4'));
}

/// Bug de 21/11: a sessão foi aberta e fechada sem nenhuma mensagem, o id ficou salvo e
/// cada mensagem seguinte dava error_during_execution no --resume.
#[test]
fn nao_retoma_sessao_que_nunca_foi_gravada() {
    let _g = preparar();
    let fantasma = sessao::uuid4();
    sessao::salvar_id(&fantasma);
    let (id, retomar) = sessao::escolher_sessao();
    assert!(!retomar, "tentou retomar uma sessão sem transcrição");
    assert_ne!(id, fantasma);
    sessao::esquecer_id();
}

#[test]
fn retoma_quando_a_transcricao_existe() {
    let _g = preparar();
    let id = sessao::uuid4();
    // mesmo caminho que o Claude Code usa: pasta de trabalho com tudo que não é letra/número virando '-'
    let pasta: String = pasta_projetos().to_string_lossy().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    let dir = PathBuf::from(std::env::var("USERPROFILE").unwrap()).join(".claude").join("projects").join(pasta);
    std::fs::create_dir_all(&dir).unwrap();
    let arq = dir.join(format!("{id}.jsonl"));
    std::fs::write(&arq, "{}\n").unwrap();
    sessao::salvar_id(&id);
    let r = sessao::escolher_sessao();
    let _ = std::fs::remove_file(&arq);
    sessao::esquecer_id();
    assert_eq!(r, (id, true));
}

// ---------- textos do cartão de aprovação ----------
#[test]
fn resumo_e_nomes_de_ferramentas() {
    assert_eq!(sessao::resumo_ferramenta("PowerShell", &json!({"command": "ls", "description": "listar"})), "listar\nls");
    assert_eq!(sessao::resumo_ferramenta("Bash", &json!({"command": "ls"})), "ls");
    assert_eq!(sessao::resumo_ferramenta("Write", &json!({"file_path": "C:\\a.txt"})), "C:\\a.txt");
    assert_eq!(sessao::resumo_ferramenta("Outra", &json!({})), "");
    let longo = "x".repeat(2000);
    assert_eq!(sessao::resumo_ferramenta("Bash", &json!({"command": longo})).chars().count(), 600);
    assert_eq!(sessao::nome_amigavel("mcp__claude-in-chrome__navigate"), "Chrome: navigate");
    assert_eq!(sessao::nome_amigavel("mcp__claude_ai_Gmail__get_thread"), "Gmail: get thread");
    assert_eq!(sessao::nome_amigavel("Agent"), "agente");
}

#[test]
fn prompt_modelo_tem_as_regras_de_economia() {
    let p = include_str!("../prompt/prompt-modelo.md");
    for trecho in ["SEMPRE por agente", "select:", "internalDate", "[[LEMBRETE", "[[SILENCIO]]", "[[OPUS]]", "[[SONNET]]", "[[NOVO_ASSUNTO]]"] {
        assert!(p.contains(trecho), "prompt-modelo sem: {trecho}");
    }
}

// ---------- Claude Code de verdade (lento, usa a cota; rode com --ignored) ----------
struct Turno {
    textos: Vec<(Modelo, String)>,
    erro: Option<String>,
    encerrou: bool,
}

fn abrir(id: &str, retomar: bool, m: Modelo) -> (Sessao, Receiver<Evento>) {
    let (tx, rx) = channel();
    let s = Sessao::abrir(id, retomar, m, tx, Arc::new(|| {})).expect("não abriu o Claude Code");
    (s, rx)
}

/// Espera o fim do turno (ou o processo cair). Nega qualquer pedido de permissão.
fn esperar(rx: &Receiver<Evento>, limite: Duration) -> Turno {
    let t = Instant::now();
    let mut r = Turno { textos: vec![], erro: None, encerrou: false };
    while t.elapsed() < limite {
        match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(Evento::Texto { modelo, texto }) => r.textos.push((modelo, texto)),
            Ok(Evento::Permissao { resposta, .. }) => {
                let _ = resposta.send(false);
            }
            Ok(Evento::FimTurno { erro }) => {
                r.erro = erro;
                return r;
            }
            Ok(Evento::Encerrou { erro, .. }) => {
                r.encerrou = true;
                r.erro = r.erro.or(erro).or(Some("encerrou".into()));
                return r;
            }
            Ok(_) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(_) => break,
        }
    }
    panic!("turno não terminou em {limite:?}");
}

fn junta(t: &Turno) -> String {
    t.textos.iter().map(|(_, s)| s.as_str()).collect::<Vec<_>>().join("\n")
}

#[test]
#[ignore]
fn vivo_nova_retomar_e_trocar_modelo() {
    let _g = preparar();
    let id = sessao::uuid4();

    // 1) sessão nova responde e grava a transcrição
    let (mut s, rx) = abrir(&id, false, Modelo::Haiku);
    s.enviar_texto("Teste automático. Responda exatamente a palavra BANANA e nada mais.");
    let t = esperar(&rx, Duration::from_secs(120));
    assert!(t.erro.is_none(), "erro na sessão nova: {:?}", t.erro);
    assert!(junta(&t).to_uppercase().contains("BANANA"), "resposta: {}", junta(&t));
    let ctx = s.contexto.load(std::sync::atomic::Ordering::Relaxed);
    println!("contexto da sessão nova (tokens): {ctx}");
    assert!(ctx > 0, "contador de contexto não foi preenchido");
    assert!(sessao::transcricao_existe(&id), "transcrição não encontrada no caminho calculado");
    s.encerrar();

    // 2) retomar a mesma sessão lembra da conversa
    let (mut s, rx) = abrir(&id, true, Modelo::Haiku);
    s.enviar_texto("Qual palavra você respondeu na mensagem anterior? Responda só ela.");
    let t = esperar(&rx, Duration::from_secs(120));
    assert!(t.erro.is_none(), "erro ao retomar: {:?}", t.erro);
    assert!(junta(&t).to_uppercase().contains("BANANA"), "não lembrou: {}", junta(&t));

    // 3) troca de modelo no meio da sessão
    s.trocar_modelo(Modelo::Sonnet);
    s.enviar_texto("Responda só OK.");
    let t = esperar(&rx, Duration::from_secs(120));
    assert!(t.erro.is_none(), "erro depois de trocar o modelo: {:?}", t.erro);
    assert!(t.textos.iter().any(|(m, _)| *m == Modelo::Sonnet), "não respondeu como Sonnet: {:?}", t.textos.iter().map(|(m, _)| m.nome()).collect::<Vec<_>>());
    s.encerrar();
}

/// Documenta o modo de falha que o app agora evita: --resume de um id sem transcrição.
#[test]
#[ignore]
fn vivo_retomar_fantasma_da_erro_e_o_app_nao_tenta() {
    let _g = preparar();
    let fantasma = sessao::uuid4();
    let (mut s, rx) = abrir(&fantasma, true, Modelo::Haiku);
    s.enviar_texto("oi");
    let t = esperar(&rx, Duration::from_secs(60));
    assert!(t.erro.is_some(), "esperava erro ao retomar sessão inexistente");
    assert!(!sessao::transcricao_existe(&fantasma));
    // e é exatamente o caso em que escolher_sessao() cria uma nova
    sessao::salvar_id(&fantasma);
    assert!(!sessao::escolher_sessao().1);
    sessao::esquecer_id();
}

/// Reproduz o bug de 21/11 de ponta a ponta: abre, fecha sem mensagem, reabre como o app faz.
#[test]
#[ignore]
fn vivo_sessao_fechada_sem_mensagem_continua_funcionando() {
    let _g = preparar();
    let id = sessao::uuid4();
    let (s, _rx) = abrir(&id, false, Modelo::Haiku);
    sessao::salvar_id(&id);
    std::thread::sleep(Duration::from_secs(3));
    s.encerrar();

    let (id2, retomar) = sessao::escolher_sessao();
    let (mut s, rx) = abrir(&id2, retomar, Modelo::Haiku);
    s.enviar_texto("Responda só OK.");
    let t = esperar(&rx, Duration::from_secs(120));
    s.encerrar();
    sessao::esquecer_id();
    assert!(t.erro.is_none(), "erro depois de reabrir: {:?}", t.erro);
}

#[test]
#[ignore]
fn vivo_interromper_termina_o_turno() {
    let _g = preparar();
    let id = sessao::uuid4();
    let (mut s, rx) = abrir(&id, false, Modelo::Haiku);
    s.enviar_texto("Escreva os números de 1 a 400 por extenso, um por linha.");
    std::thread::sleep(Duration::from_secs(4));
    s.interromper();
    let t = Instant::now();
    let r = esperar(&rx, Duration::from_secs(60));
    println!("turno terminou {:?} depois do interromper (erro: {:?})", t.elapsed(), r.erro);
    assert!(!r.encerrou, "o processo caiu em vez de só parar o turno");
    assert!(r.erro.is_none(), "parar apareceria como erro no chat: {:?}", r.erro);
    // a sessão continua usável depois
    s.enviar_texto("Responda só OK.");
    let r = esperar(&rx, Duration::from_secs(120));
    s.encerrar();
    assert!(r.erro.is_none(), "sessão ficou quebrada depois do interromper: {:?}", r.erro);
}
