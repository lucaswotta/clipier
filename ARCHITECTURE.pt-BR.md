# Leme Pier: arquitetura

Status: proposta para a 0.0.1, a ser aprovada antes de qualquer código. [English](ARCHITECTURE.md)

## O que é

O Leme Pier é um app de desktop que encontra os CLIs de IA para programar já instalados e com login na sua máquina (`claude`, `codex`, `agy`, `opencode`) e deixa você usá-los num só lugar:

- **Chat.** Uma conversa que você pode continuar com outro agente ou modelo sem começar de novo. O próximo agente recebe um briefing do que aconteceu; ninguém relê o repositório inteiro.
- **Regata.** Um pedido enviado a até quatro pares de agente e modelo ao mesmo tempo. Cada um trabalha na sua própria cópia do projeto. Você lê os resultados e fica com um.
- **Tripulação.** Um pedido dividido por um capitão em partes. Cada parte vai para um papel (construtor, revisor, ...), as partes que não dependem uma da outra rodam ao mesmo tempo, e tudo é reunido numa só branch.
- **Memória.** Notas sobre um projeto que todo agente lê e pode acrescentar.

Ele não chama nenhuma API de modelo. Inicia os CLIs oficiais com os logins que você já fez.

O nome é o cais onde os seus agentes se encontram; o executável é `leme`. A tagline é "Every agent, one dock" ("Todos os agentes, um só cais").

### Fora do escopo

Sem serviço hospedado, sem contas, sem telemetria, sem chaves de API de modelo, sem sistema de plugins, sem motor de agendamento, sem execuções autônomas em segundo plano. É uma ferramenta diante da qual você se senta.

## Princípios

1. **Pequeno.** Meta de 4 a 5 mil linhas de Rust e cerca de 2 mil de JavaScript, sem contar testes. Uma funcionalidade que exige um subsistema novo é uma funcionalidade que recusamos.
2. **Explicável.** Cada modo precisa ser explicável a uma criança de doze anos em duas frases. Se não der, o modo é redesenhado, não documentado.
3. **Uma Regata ou uma Tripulação nunca toca no seu checkout.** Os agentes delas trabalham em git worktrees e o que produzem fica em branches até você decidir. O Chat é a exceção, como num terminal: o agente edita a pasta que você apontou, e a tela mostra o estado do repositório para você ver o que mudou.
4. **Quem pensa são os agentes.** O Leme não escolhe nada pelo modelo: sem prompts escondidos que mudem o comportamento, sem novas tentativas que gastem token em silêncio. Ele encaminha, isola, registra e informa.
5. **Só local.** Tudo é um arquivo no seu disco. Nada sai da máquina além do que os próprios CLIs dos agentes enviam.
6. **Aberto desde o primeiro commit.** Apache-2.0. O histórico é escrito para ser lido.

## Vocabulário

A interface é náutica; o código usa o termo simples e a interface faz o mapeamento.

| Termo simples (código) | Mostrado como | Significado |
|---|---|---|
| agent | agente | Um dos CLIs: `claude`, `codex`, `agy`, `opencode` |
| model | modelo | O modelo que um agente roda; vazio significa o padrão do CLI |
| conversation | conversa | Um registro de chat, guardado pelo Leme |
| brief | o pedido | O que o usuário pede |
| race / boat | Regata / barco | Um pedido a vários agentes em paralelo / um deles |
| crew / piece | Tripulação / parte | Um pedido executado por um plano / um passo do plano |
| captain | capitão | O papel que escreve o plano |
| role | papel | Uma função com nome: quem faz (agente, modelo) e como é instruído |
| note | nota | Um fato sobre um projeto que os agentes compartilham |
| check | verificação | O comando do próprio projeto que diz se o trabalho está bom (os testes dele) |

## Layout do repositório

```
leme-pier/
  Cargo.toml                 workspace
  crates/
    core/                    tudo o que não é janela (biblioteca, totalmente testável)
      src/
        platform/            o único lugar que distingue Linux de Windows
        agents/              um adaptador por CLI + detecção
        runner.rs            inicia um processo, vigia, para
        usage.rs             o que cada agente fez (registro próprio do Leme)
        session/             conversas e o briefing
        memory.rs            notas do projeto
        git.rs               worktrees, commits, merges, diffs, escopo
        check.rs             achar e rodar a verificação de um projeto
        roles.rs             arquivos de papéis: de fábrica, do usuário, do projeto
        plan.rs              o formato do plano e a validação
        race.rs              a Regata
        crew.rs              a Tripulação
        store.rs             onde as coisas ficam no disco
  app/
    src-tauri/               casca Tauri: só comandos, sem lógica
    web/                     front-end: módulos JS puros, sem framework
  docs/                      ARCHITECTURE, SECURITY, CONTRIBUTING, ROADMAP
  scripts/verify.sh          fmt, clippy -D warnings, testes, build do front-end
```

O `core` não depende do Tauri. A casca transforma cada função pública do `core` em um comando e nada mais; se a casca ganhar lógica, a lógica vai para o `core`.

### Orçamento de tamanho (Rust, sem testes)

| Módulo | Linhas |
|---|---|
| `agents` (4 adaptadores, leitura da saída, detecção) | 1.300 |
| `runner`, `platform` | 450 |
| `session`, `memory`, `usage` | 700 |
| `git`, `check` | 650 |
| `roles`, `plan` | 400 |
| `race`, `crew` | 700 |
| `store` e cola | 200 |
| `app/src-tauri` | 500 |
| **Total** | **cerca de 4.900** |

Para comparar, o protótipo tem 13.600 linhas. Ele fica como arquivo privado.

## Módulos do núcleo

### `agents`

```text
trait Agent {
    fn id(&self) -> &'static str;
    fn detect(&self) -> Detected;                       // instalado, versão, login, modelos
    fn command(&self, request: &Request) -> Spawn;      // programa, args, env, cwd
    fn parse(&self, line: &str, into: &mut Transcript); // uma linha da saída do CLI
}
```

A detecção pergunta ao próprio CLI, sem chamar modelo, e foi conferida à mão contra os quatro CLIs na máquina do autor:

| CLI | Modelos | Uso e limites |
|---|---|---|
| `claude` | `claude -p "/model"` imprime os apelidos que aceita (lidos do texto) | `claude -p "/usage"` imprime a porcentagem usada da sessão e da semana e quando cada uma zera |
| `codex` | `codex debug models` imprime o catálogo em JSON (slug, nome, níveis de raciocínio, visibilidade) | a investigar |
| `agy` | `agy models` imprime id e nome por linha | só se sabe quando uma chamada falha ("quota reached, resets in ..."), a menos que se ache um comando |
| `opencode` | `opencode models` imprime `provider/model` por linha | a investigar |

Comandos de barra funcionam sem interação no Claude; para os outros, a tabela lista o que existe. Cada leitor é testado contra uma saída gravada, e um CLI cujo comando deixe de funcionar degrada para "só o modelo padrão" em vez de falhar.

Cada adaptador é um arquivo e carrega o que foi caro descobrir: como o CLI nomeia uma sessão, como a retoma, como pede um sandbox, como informa tokens, como é a saída. Um CLI que muda de formato quebra um arquivo e um teste. O `Transcript` reúne os eventos (mensagem, comando, edição, ferramenta, erro), a resposta final, o id da sessão e a contagem de tokens.

### `runner`

Inicia um `Spawn`, copia a saída dele para um arquivo à medida que chega (assim uma execução morta ainda deixa o que imprimiu), passa a saída ao leitor do adaptador e impõe três limites: tempo total, silêncio e cancelamento. Para a árvore inteira de processos. As falhas são classificadas uma vez, aqui: inutilizável (sem login, modelo desconhecido), limitada (cota, com a hora em que zera quando o CLI diz) ou transitória. Uma cota que zera em horas conta como inutilizável na execução, não como algo a esperar.

### `platform`

O único módulo com `cfg(target_os)`. Fornece: parar uma árvore de processos, as pastas de dados e de configuração, achar um CLI no `PATH` (inclusive os atalhos `.cmd` no Windows) e o sandbox da verificação (veja Segurança). Nenhum outro código pode importar `std::os::unix`.

### `session`

Uma conversa é uma pasta: `meta.json` (repositório, título, o id próprio de cada agente, quanto do registro cada agente já viu) e `log.jsonl` (mensagens do usuário, respostas dos agentes com arquivos editados e comandos, falhas).

Quando um agente fala, recebe uma de duas coisas:

- ele falou por último e tem sessão própria: a mensagem simples, retomada na sessão dele, sem briefing;
- caso contrário, um **briefing** montado sem modelo: o que ele não viu (entradas mais novas primeiro, com limite), o estado do repositório (`git status`, tamanho do diff) e as notas do projeto.

Uma resposta pode terminar com linhas `NOTE: título | texto`. Elas saem da resposta e vão para a memória.

### `memory`

Arquivos markdown por repositório, uma nota por arquivo, com front matter. Texto que parece segredo é recusado, duplicatas são descartadas, cada turno pode acrescentar poucas notas e o usuário pode esquecer qualquer uma.

### `usage`

Um registro só de acréscimo do que cada agente fez pelo Leme: agente, modelo, papel, tokens (novos, em cache, saída), duração, resultado. A tela de Agentes lê dele. Substitui as estatísticas que o protótipo lia da ferramenta antiga.

### `git`

Worktrees na pasta de dados, uma por barco ou parte, a partir do commit inicial; commit com identidade de reserva; merge de uma parte na branch de integração; diff; arquivos alterados; verificação de escopo (glob de verdade) para que uma parte que escreveu fora dos caminhos declarados seja apontada; pastas de ruído (`node_modules`, `target`, ...) ignoradas. Consegue afirmar que o checkout do usuário está exatamente como estava.

### `check`

Detecta o comando de teste do projeto (`cargo test`, `npm test`, `pytest`, `go test`, `make test`, ...) ou usa um de `.leme-pier/config.toml` ou do formulário. Roda numa worktree com limite de tempo, guarda o fim da saída e informa passou ou falhou.

## Papéis

Um papel responde: quem faz este trabalho e como é instruído. Um papel é um arquivo markdown.

```markdown
---
name: builder
title: Construtor naval    # nome mostrado; a interface pode traduzir os de fábrica
mode: write                # read | write
agent: codex
model: ""                  # vazio = o padrão do CLI
fallback:                  # opcional, só um
  agent: claude
  model: sonnet
---
Você implementa a parte que recebeu dentro desta worktree. Mude só o que a parte precisa.
Ao terminar, responda com um resumo curto do que mudou.
```

- Os papéis **de fábrica** vêm com o app: `captain` (lê), `builder` (escreve), `reviewer` (lê), `tester` (lê). Existem em inglês, português e espanhol.
- Papéis **do usuário**: `~/.config/leme-pier/roles/*.md`. Papéis **do projeto**: `.leme-pier/roles/*.md`, versionados com o código. Um arquivo com o mesmo `name` sobrescreve o anterior (projeto sobre usuário sobre fábrica).
- A **reserva** é usada uma vez, e só quando o agente está inutilizável ou limitado, nunca por uma resposta ruim. É a única recuperação automática do app.
- Um papel que cita um agente não instalado aparece como indisponível na tela de Agentes e não pode ser atribuído.

A escolha de modelos por papel acontece numa tela: uma linha por papel com um seletor de agente e um de modelo. Ela grava o arquivo do usuário.

### Configuração do projeto

`.leme-pier/config.toml`, opcional:

```toml
check = "cargo test"       # o que uma parte precisa passar
captain = "captain"        # qual papel planeja
approve_plan = true        # false: a tripulação começa assim que o plano é válido
parallel = 3               # partes que podem rodar ao mesmo tempo
```

## Regata

1. Entrada: repositório, pedido, de 1 a 4 barcos (agente e modelo cada), verificação opcional.
2. Cada barco recebe uma worktree e roda em sua própria thread com o pedido, uma frase dizendo que outros trabalham na mesma tarefa em outro lugar e as notas do projeto.
3. Quando um barco termina: commit, registro dos arquivos alterados, execução da verificação, registro do veredito.
4. Saída por barco: resumo, arquivos, veredito da verificação, tokens, duração, nome da branch, diff. As branches são `leme/<run>/<boat>`. A tela mostra os comandos para inspecionar e reunir uma.
5. Nada é integrado automaticamente. As worktrees são removidas no fim; as branches ficam.

"Melhorar meu pedido" é um turno separado, opcional, só de leitura, que reescreve o pedido; o usuário pode desfazer e editar. Nunca é aplicado em silêncio.

## Tripulação (o modo simples)

A tripulação executa um plano. Não há agendador, nem política de novas tentativas, nem motor de orçamento.

### O plano

O papel capitão devolve JSON, validado antes de qualquer coisa começar:

```json
{
  "summary": "Acrescentar o comando de exportação em dois passos, depois revisar.",
  "pieces": [
    { "id": "schema", "role": "builder", "title": "Definir o formato de exportação",
      "brief": "...", "touches": ["src/export/format.rs"], "after": [] },
    { "id": "command", "role": "builder", "title": "Acrescentar o comando",
      "brief": "...", "touches": ["src/cli/export.rs"], "after": ["schema"] },
    { "id": "look", "role": "reviewer", "title": "Olhar o comando",
      "brief": "...", "touches": [], "after": ["command"], "reviews": "command" }
  ]
}
```

Regras, todas conferidas em `plan.rs`: os ids são únicos; todo `role` existe e está disponível; `after` aponta para partes existentes e não tem ciclo; no máximo 12 partes; duas partes de escrita que podem rodar ao mesmo tempo não podem se sobrepor em `touches`; uma parte `reviews` aponta uma parte de escrita e usa um papel de leitura. Um plano que quebra uma regra é recusado com o motivo e o capitão é chamado uma vez para corrigir.

### Execução

O usuário lê o plano e aprova (`approve_plan = false` pula a leitura; o plano continua validado e continua sendo mostrado enquanto roda). Depois, num laço:

1. Uma parte está **pronta** quando tudo em seu `after` foi reunido (ou, para uma revisão, terminou).
2. As partes prontas começam, até `parallel` por vez (padrão 3), cada uma numa worktree criada a partir da branch de integração como ela está agora. Uma parte também recebe os resumos das partes de que depende (`after`), já que o código reunido não diz por que foi escrito daquele jeito.
3. Uma parte de escrita terminada é commitada, tem o escopo conferido, passa pela verificação e é reunida na branch de integração (`leme/<run>/integration`). Um conflito nunca é forçado.
4. Uma parte de revisão terminada devolve `approved` ou uma lista de mudanças. Mudanças mandam a parte revisada de volta uma vez, com as notas; uma segunda reprovação pergunta ao usuário.
5. A execução termina quando toda parte foi reunida, pulada ou parada.

### O que pergunta ao usuário

Exatamente três situações, cada uma com as mesmas escolhas (refazer, pular, parar):

- uma parte falhou (erro do agente, ou a verificação falhou depois de uma nova tentativa);
- uma parte não se junta limpa;
- uma revisão reprovou uma parte duas vezes.

Todo o resto segue. Um botão de cancelar para todos os processos.

## Dados no disco

| O quê | Onde (Linux) |
|---|---|
| Conversas | `$XDG_DATA_HOME/leme-pier/conversations/<id>/` |
| Regatas e tripulações | `.../runs/<id>/` (`run.json`, `events.jsonl`, `out/`) |
| Worktrees | `.../worktrees/<run>/<piece>/` |
| Memória | `.../memory/<repo-key>/<note>.md` |
| Uso | `.../usage.jsonl` |
| Papéis e ajustes do usuário | `$XDG_CONFIG_HOME/leme-pier/` |
| Ajustes e papéis do projeto | `.leme-pier/` no repositório |

Todo arquivo JSON leva um campo `version`. Um leitor que encontra uma versão mais nova a recusa e diz isso. As gravações passam por um arquivo temporário e um rename. Uma linha cortada por uma queda é ignorada na leitura.

## Modelo de segurança

O que o Leme garante:

- O checkout, a branch e o índice do usuário não são tocados; o Leme consegue verificar isso e avisa quando não consegue.
- Cada barco e cada parte escreve só na sua worktree. O `touches` de uma parte de escrita é conferido depois: mudanças fora do escopo são apontadas e bloqueiam a reunião.
- A verificação do projeto roda sem segredos no ambiente, com limite de tempo e, onde a plataforma tem, num sandbox sem rede (bubblewrap).
- Variáveis de ambiente que parecem segredos são removidas de todo processo de agente.
- O Leme nunca inicia uma execução de dentro de uma execução que ele mesmo iniciou (uma variável de guarda), então um agente não consegue criar uma tripulação recursivamente.
- Notas que parecem credenciais nunca são guardadas.

O que ele não garante, e diz isso no `SECURITY.md`:

- Os agentes rodam com os seus logins e as suas permissões. O Leme pede a cada CLI o modo que aceita edições e tem sandbox quando existe; não substitui a segurança do próprio CLI.
- Onde não há sandbox (Windows), a verificação roda sem sandbox. A interface avisa.

## O app

Tauri 2. A webview carrega só arquivos locais; as capabilities concedem os comandos e nada mais (um teste afirma que as APIs centrais são negadas). A interface consulta a casca para ler o estado em vez de assinar eventos, o que mantém a superfície de permissões mínima.

Telas (primeira versão): **Chat**, **Set course** (Regata e Tripulação), **Agentes** (instalados, com login, versão, modelos, uso, papéis com seletores de agente). A barra lateral lista as execuções passadas. Uma paleta de dia e uma de noite. O idioma segue o do sistema: inglês, português do Brasil, espanhol, com inglês como reserva; não há seletor.

Módulos do front-end: `chat`, `course` (formulário, vista da regata), `crew` (plano, execução), `agents`, `fleet` (a lista de agentes compartilhada), `i18n` com um catálogo por idioma, `markdown` (um renderizador pequeno e seguro), `style.css`. Sem framework, sem biblioteca de estado.

## Plataformas

- **Linux** é completo na 0.0.1: `.deb` e AppImage.
- **Windows** é o próximo alvo; o `platform` já tem o formato para ele (árvore de processos por Job Objects, resolução de `.cmd`, `AppData`, sem bubblewrap). O CI compila e testa no Windows desde a primeira versão para não apodrecer; rodar agentes de verdade lá é verificado à mão.
- **macOS** depois do Windows; exige conta de desenvolvedor paga para notarizar.

## Testes

Os testes existem para pegar o que machucaria um usuário. Não medem esforço, e não há meta de quantidade. As regras:

- **Um teste nomeia um comportamento.** "Uma cota que zera em horas torna o agente inutilizável" é um teste. Um teste que repete o código, confere um getter ou existe para aumentar um número é apagado.
- **Rápidos por construção.** A suíte padrão inteira roda em menos de 30 segundos. Um teste que precisa de mais tempo não pertence a ela. `scripts/verify.sh quick` (formatação, clippy, os testes do crate em mudança) é o que roda enquanto se trabalha; o `verify.sh` completo roda no CI.
- **O que é testado, e como:**
  - os quatro leitores de saída e a detecção de modelos e uso, cada um contra uma saída gravada do CLI real;
  - o runner (limites, cancelamento, árvore de processos) com scripts de shell minúsculos como agentes de mentira;
  - as operações de git (worktrees, commit, merge, escopo, conflito) num repositório temporário;
  - a validação do plano (ciclos, sobreposição, papéis desconhecidos), as sessões e o briefing, o filtro da memória;
  - um teste ponta a ponta de uma Regata e um de uma Tripulação com agentes de mentira.
- **Fora da suíte padrão:** agentes reais, benchmarks e o self-test da janela. Os testes com agentes reais são marcados `#[ignore]` e rodados à mão. O self-test da janela (`LEME_PIER_SELFTEST=<arquivo>`, cobrindo os três idiomas) roda quando a interface muda e antes de uma versão, não a cada commit.
- Um teste IPC do Tauri confere que os comandos funcionam e que as APIs não permitidas são negadas.
- Nenhum benchmark vai na 0.0.1. Se um briefing economiza tokens é medido depois que o app estiver utilizável; o método e os resultados vão para `docs/`.

## Versão e projeto

- Versão `0.0.1`; versionamento semântico, com 0.x significando "pode quebrar". O GitHub Releases leva os pacotes e um changelog.
- Licença: Apache-2.0. `CONTRIBUTING.md` (rodar o verify antes de um PR, inglês no código e nos docs, commits como Conventional Commits), `SECURITY.md` (o modelo acima e como reportar), modelos de issue e de PR.
- Sem telemetria. O app não faz nenhuma requisição de rede por conta própria.

## O que o protótipo tinha e isto não tem

| Protótipo | Destino |
|---|---|
| `scheduler`, `orchestrator`, `planner` | Substituídos por `plan` e `crew`: sem máquina de estados, sem motor de políticas |
| Cadeias de papéis, orçamentos, limites, estatísticas por papel da ferramenta antiga | Substituídos por arquivos de papéis com uma reserva; registro próprio de `usage` |
| `legacy` (lê o harness antigo), On deck, Logbook, a gaveta da execução | Removidos |
| `bench`, `headless run`, `gc`, a CLI `voyages` | Removidos; uma CLI pode vir depois sobre o `core` |
| Cadeias de revisão entre famílias de modelos | Mantidas como o papel `reviewer`, escolhido pelo usuário |

## Plano de commits

Cada commit compila e passa no `scripts/verify.sh` sozinho.

1. Workspace, licença, CI, `verify.sh`, README mínimo.
2. `platform` e `runner`.
3. `agents`: trait, `Transcript`, os quatro adaptadores e a detecção, com saídas gravadas.
4. `git` e `check`.
5. `memory` e `usage`.
6. `session`: conversas e o briefing.
7. `roles` e `plan`.
8. `race`.
9. `crew`.
10. A casca Tauri e seus comandos, com o teste IPC.
11. Base do front-end: layout, tema, i18n (três catálogos), a lista de agentes.
12. Tela de Chat.
13. Set course: formulário e Regata.
14. Set course: Tripulação.
15. Tela de Agentes e seletores de papéis.
16. Self-test da janela, empacotamento (`.deb`, AppImage), docs, `0.0.1`.

## Riscos e pontos em aberto

- **Nome.** Uma empresa chamada Leme (leme.ai) lança um produto de IA e tem um repositório de CLI para agentes de código. A consulta de marca (INPI e outras) está pendente, e o nome fica num só lugar para poder mudar.
- **Termos de uso.** Os termos dos fornecedores de agentes sobre dirigir os CLIs oficiais por outro programa precisam ser lidos antes de anunciar o repositório.
- **Deriva de formato.** Os CLIs mudam a saída. Os adaptadores e seus testes com saída gravada são a defesa; uma quebra é um conserto de um arquivo.
- **A promessa a provar.** Que um briefing deixa o próximo agente mais barato que começar do zero é uma suposição com um único dado. É medida depois da 0.0.1 e reportada nos dois casos.
- **Windows.** O `agy` e alguns CLIs podem não rodar lá; só se saberá testando.
