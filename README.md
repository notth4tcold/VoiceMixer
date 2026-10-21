# VoiceMixer

Mesa de mixagem virtual para Linux (PipeWire), escrita em Rust. Ela reproduz o
funcionamento do Voicemeeter Banana: 5 entradas, 5 saídas, roteamento em matriz,
efeitos, gravador, áudio pela rede (VBAN), MIDI e controle remoto.

> Implementação própria, feita a partir de documentação, dos arquivos de configuração
> e do comportamento observável do programa. Nenhum código do Voicemeeter foi
> descompilado ou copiado. "Voicemeeter" e "VBAN" são da VB-Audio Software; o VBAN
> segue a especificação pública deles.

## Recursos

| Área | O que tem |
|---|---|
| Entradas | 3 de hardware (estéreo) + 2 virtuais (até 7.1): **VoiceMixer Input** e **VoiceMixer AUX** |
| Saídas | A1–A3 de hardware + B1–B2 virtuais (aparecem como microfones **VoiceMixer B1/B2**) |
| Roteamento | Botões A1/A2/A3/B1/B2 em cada faixa; mute, solo, mono, fader -60…+12 dB |
| Faixa de hardware | Painel de cor (tilt/presença/calor), painel 3D (pan/largura), chorus, compressor com make-up automático, gate com hold e filtro no detector, limitador |
| Faixa virtual | EQ de 3 bandas, posição surround, karaokê (4 métodos), mute do canal central |
| Saídas (buses) | EQ paramétrico de 6 bandas por canal (com curva arrastável), mono, mute, 12 modos multicanal (Mix Down A/B, Stereo Repeat, Composite, TV Mix, Up Mix 2.1/4.1/6.1, Center/LFE/Rear Only) |
| Gravador (Tape) | Grava WAV 16/24/32f ou MP3 (via `lame`); toca WAV, FLAC, MP3, AAC/M4A, Ogg, ALAC e o áudio de MP4/MKV; REW/FF/loop |
| VBAN | 8 streams de entrada e 8 de saída (PCM16/24, float32), comandos de texto (VBAN-TEXT) |
| MIDI | Mapeamento com "learn" (CC ou nota; absoluto, toggle, momentâneo) |
| Controle | `voicemixer ctl`, botões de macro, MIDI e VBAN-TEXT, todos com a mesma sintaxe |
| Configuração | Salva sozinha em `~/.config/voicemixer/settings.toml`; presets; importa o XML do Voicemeeter Banana |

## Instalar

Baixe a última versão da sua distro em [Releases](https://github.com/notth4tcold/VoiceMixer/releases/latest):

```sh
# Ubuntu / Debian
wget https://github.com/notth4tcold/VoiceMixer/releases/latest/download/voicemixer_amd64.deb
sudo apt install ./voicemixer_amd64.deb

# Arch
wget https://github.com/notth4tcold/VoiceMixer/releases/latest/download/voicemixer-x86_64.pkg.tar.zst
sudo pacman -U voicemixer-x86_64.pkg.tar.zst
```

O `.deb` e o `.tar.gz` precisam de Ubuntu 24.04 / Debian 13 ou mais novo (PipeWire 1.0+).
O `.tar.gz` funciona em qualquer distro x86_64 recente com PipeWire. No Arch, também dá
para compilar do código-fonte com o `packaging/arch/PKGBUILD` (`makepkg -si`).

## Compilar no Linux

Dependências de sistema:

```sh
# Debian / Ubuntu
sudo apt install build-essential pkg-config clang libclang-dev libpipewire-0.3-dev libspa-0.2-dev libasound2-dev
# Fedora
sudo dnf install gcc pkgconf clang clang-devel pipewire-devel alsa-lib-devel
# Arch
sudo pacman -S base-devel clang pipewire alsa-lib
```

Rust: <https://rustup.rs>. Depois:

```sh
cargo build --release
./target/release/voicemixer
```

Sem MIDI (dispensa a `libasound2-dev`): `cargo build --release --no-default-features`.

Para instalar: `install -Dm755 target/release/voicemixer ~/.local/bin/voicemixer`
e `install -Dm644 packaging/voicemixer.desktop ~/.local/share/applications/voicemixer.desktop`.

## Primeiros passos

1. Em **Hardware Input 1**, escolha o microfone. Em **Hardware Out → A1**, escolha fones ou caixas de som.
2. Nas configurações de som do sistema, use **VoiceMixer Input** como saída padrão.
   Os apps também podem ser direcionados um a um (`pavucontrol`, `qpwgraph`, Helvum).
3. No Discord, OBS etc., use **VoiceMixer B1** como microfone.
4. Os botões A/B de cada faixa decidem quem ouve o quê. Por exemplo: microfone → B1
   (só o Discord ouve), Spotify → A1 + B1 (você e o Discord ouvem).

Para importar um setup do Windows, copie `Documents\Voicemeeter\VoicemeeterBanana_LastSettings.xml`
e use **Menu → Import Voicemeeter settings**, ou `voicemixer --import-banana arquivo.xml`.
Os dispositivos não são importados (os nomes do Windows não existem no Linux).

## Controle remoto e scripts

```sh
voicemixer ctl 'Strip[0].Gain = -6; Strip[0].B1 = 1'
voicemixer ctl 'Bus[0].Gain += 3'
voicemixer ctl get Strip[0].Mute
voicemixer ctl levels            # picos em dB: 5 faixas e 5 buses
voicemixer --headless            # sem janela, só controle remoto
```

O socket fica em `$XDG_RUNTIME_DIR/voicemixer.sock` (uma linha por comando; resposta `OK …` ou `ERR …`).
A mesma sintaxe funciona em macros, MIDI e VBAN-TEXT. Os índices começam em 0.

| Parâmetro | Valores |
|---|---|
| `Strip[i].Gain` | -60…12 dB |
| `Strip[i].Mute` / `Solo` / `Mono` | 0/1 |
| `Strip[i].A1` … `B2` | 0/1 |
| `Strip[i].Label`, `Strip[i].Device` | texto (`Device` = `node.name` do PipeWire; só faixas 0–2) |
| `Strip[i].Comp`, `Strip[i].Gate` | 0…10 (faixas 0–2) |
| `Strip[i].Comp.GainIn/Threshold/Attack/Release/Knee/MakeUp/GainOut` | detalhes do compressor |
| `Strip[i].Gate.Damping/BPSidechain/Attack/Hold/Release` | detalhes do gate |
| `Strip[i].Limit` | -40…12 dB (12 = desligado) |
| `Strip[i].Pan_x/Pan_y/Color_x/Color_y/fx_x/fx_y` | -1…1 |
| `Strip[i].EQGain1/2/3` | -12…12 dB (graves/médios/agudos; faixas 3–4) |
| `Strip[i].Karaoke` | 0…4; `Strip[i].MC` 0/1 (faixas 3–4) |
| `Bus[i].Gain/Mute/Mono/Label/Device` | como nas faixas (`Device` só em A1–A3) |
| `Bus[i].Mode` | `normal amix bmix repeat composite tvmix upmix21 upmix41 upmix61 centeronly lfeonly rearonly` |
| `Bus[i].mode.Amix = 1` | forma alternativa |
| `Bus[i].EQ.on`, `Bus[i].EQ.channel[c].cell[k].on/type/gain/f/q` | EQ paramétrico |
| `Recorder.Play/Stop/Record/Pause/FF/Rew/Eject = 1` | transporte |
| `Recorder.Load = "/caminho/arquivo.mp3"` | carregar arquivo |
| `Recorder.Gain`, `Recorder.A1`…`B2`, `Recorder.Loop`, `Recorder.Format` | gravador |
| `Recorder.ArmStrip[i]`, `Recorder.ArmBus[i]` | o que é gravado |
| `vban.Enable`, `vban.instream[i].on/name/ip/route`, `vban.outstream[i].on/name/ip/port/route/channel/format` | VBAN |
| `Patch.Composite[c]` | 0…22 (origem de cada canal no modo Composite) |
| `Button[i].State = 1/0` | aciona um botão de macro |
| `Command.Save = "nome"`, `Command.Load = "nome"`, `Command.Reset = 1`, `Command.Restart = 1` | presets e dispositivos |
| `Option.Lock` | trava a interface |

## Arquitetura

```
crates/voicemixer-core   independente de plataforma, com testes
  params.rs     modelo de parâmetros (serde)
  dsp.rs        biquads, compressor, gate, limitador, chorus, karaokê, modos de bus
  engine.rs     faixas → matriz → buses; FIFOs por dispositivo
  script.rs     linguagem de controle
  vban.rs       protocolo VBAN
  banana.rs     importação do XML do Voicemeeter
crates/voicemixer        o aplicativo
  backend/pipewire.rs   dispositivos virtuais e de hardware (só Linux)
  backend/null.rs       backend silencioso para testes
  gui/                  interface egui
  tape.rs  vban_net.rs  midi.rs  remote.rs
```

Cada saída que precisa de áudio faz o motor processar um bloco para todos os buses de uma
vez, então todas as saídas andam juntas. Os callbacks do PipeWire rodam na thread do main
loop, sem `RT_PROCESS`.

## Testar sem PipeWire

`VOICEMIXER_NULL_BACKEND=1 VOICEMIXER_TEST_TONE=1 voicemixer` roda sem áudio real, com
um tom de 440 Hz nas entradas 1 e 4, para ver medidores e roteamento funcionando.
Fora do Linux esse é o único backend.

## Limitações conhecidas

- Dispositivos de hardware com relógios diferentes não são ressincronizados: quando
  há deriva, o motor descarta ou repete blocos, o que pode gerar um estalo ocasional.
- O resampler (VBAN e player) é linear: bom para monitorar, não para masterização.
- Gravação em MP3 precisa do programa `lame` instalado.
- Mudanças de taxa de amostragem, buffer e número de canais exigem reiniciar o programa.
- Não implementado: os apps separados do pacote Voicemeeter (GEQ de 15 bandas,
  matriz 8×8, VBAN2MIDI, Streamer View) e o "insert" virtual de ASIO.
