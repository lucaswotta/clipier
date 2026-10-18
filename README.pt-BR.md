<p align="center"><img src="docs/images/logo.svg" width="72" alt="Leme Pier"></p>

# Leme Pier

**Todos os agentes, um só cais.**

O Leme Pier é um app de desktop que encontra os CLIs de IA para programar já instalados e com login na sua máquina (`claude`, `codex`, `agy`, `opencode`) e deixa você usá-los num só lugar. Ele inicia os CLIs oficiais com os logins que você já fez, não envia nada para lugar nenhum por conta própria e não guarda conta.

![Uma conversa que passou de um agente para outro](docs/images/chat.png)

## Por quê

Se você paga por mais de um agente de código, conhece o custo de trocar: o próximo agente precisa reler o repositório inteiro, e não dá para fazer a mesma pergunta a três deles e comparar sem esforço. O Leme Pier guarda um só registro do que foi dito e feito, então o próximo agente é posto a par do que perdeu em vez de começar do zero, e roda agentes lado a lado, cada um na sua própria cópia do projeto.

## O que ele faz

- **Conversa.** Uma conversa que você pode continuar com outro agente ou modelo. Cada resposta diz qual agente e modelo a escreveu. Quando o agente muda, o próximo recebe um briefing curto montado sem chamar modelo: o que ele não viu, o estado do repositório e as notas do projeto. O agente que falou por último retoma a própria sessão, sem briefing.
- **Regata.** Um pedido enviado a até quatro pares de agente e modelo ao mesmo tempo. Cada um trabalha na sua própria git worktree. A verificação do projeto (os testes dele) roda em cada resultado, e o trabalho espera numa branch por barco: você lê o que cada um fez e fica com um.
- **Tripulação.** Um papel de capitão divide o pedido em partes. Você lê o plano, e então as partes rodam lado a lado onde não dependem uma da outra, cada uma na sua worktree. Uma parte é verificada, pode ser revisada e é reunida numa branch de integração. Quando algo falha, não se junta limpo ou é devolvido duas vezes, ele pergunta a você; nada mais pergunta.
- **Notas.** Fatos sobre o seu projeto que os agentes deixam para o próximo (um comando, uma convenção), guardados como arquivos markdown que você pode ler e apagar.
- **Agentes.** O que cada CLI diz de si: versão, login, modelos, quanto da cota do Claude foi usada, o que passou pelo Leme e quem faz cada papel.

![Um pedido, três barcos](docs/images/race.png)

![Um plano que o capitão traçou, para ler antes de qualquer coisa começar](docs/images/plan.png)

## Requisitos

- Linux (o Windows é o próximo alvo e é compilado pelo CI, mas ainda não foi usado com agentes de verdade).
- `git` e pelo menos um dos CLIs de agente, com login.
- `bubblewrap` (`bwrap`) é recomendado: deixa a verificação do projeto rodar sem rede, e é necessário para usar `agy` e `opencode`, que rodam comandos sem perguntar.

## Instalação

Ainda não há versão publicada. Para compilar:

```sh
git clone https://github.com/lucaswotta/leme-pier
cd leme-pier/app
npm ci && npm run build
npx tauri build --bundles deb     # um .deb em ../target/release/bundle/deb/
```

Ou rode a partir do código com `npm run tauri dev`. Precisa de Rust (stable) e das bibliotecas de sistema do Tauri; veja [CONTRIBUTING.md](CONTRIBUTING.md).

## Seguro por construção

Uma Regata ou uma Tripulação nunca toca no seu checkout, na sua branch nem no seu índice: os agentes trabalham em git worktrees, e o resultado é verificado no fim. Uma parte só pode mudar os arquivos que o plano lista. A verificação roda sem segredos e, onde há sandbox, sem rede. Notas que parecem credenciais são recusadas. Veja [SECURITY.md](SECURITY.md) para o que é e o que não é garantido; uma Conversa, por exemplo, edita a pasta que você apontou, como o CLI faria num terminal.

## O que ele não faz

Sem contas, sem telemetria, sem requisição de rede própria, sem chaves de API de modelo, sem plugins, sem execuções em segundo plano. É uma ferramenta diante da qual você se senta.

## Idiomas

Inglês, português do Brasil e espanhol, seguindo o idioma do sistema (inglês quando não for nenhum deles).

## Status

Inicial: a primeira versão é a 0.0.1. O [ARCHITECTURE.pt-BR.md](ARCHITECTURE.pt-BR.md) diz como ele é construído e por que é pequeno de propósito. Se o briefing realmente deixa o próximo agente mais barato que começar do zero é uma suposição com um único dado até agora; será medido e o resultado publicado nos dois casos.

Se o Leme Pier economizar o seu tempo, uma estrela ajuda outras pessoas a encontrá-lo. Issues e pull requests são bem-vindos: veja [CONTRIBUTING.md](CONTRIBUTING.md).

## Licença

Apache-2.0. Veja [LICENSE](LICENSE).

[English version](README.md)
