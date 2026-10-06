# meerkat

Piccolo widget desktop (Rust + [egui](https://github.com/emilk/egui)) che tiene d'occhio
una lista di repository git locali: periodicamente, o premendo il pulsante di reload,
esegue `git fetch` e mostra lo stato di sincronizzazione del branch corrente rispetto
al suo upstream su `origin`.

![screenshot](docs/screenshot.png)

| Spia | Significato |
|------|-------------|
| 🟢 verde  | in sync con l'upstream |
| 🔵 blu    | hai commit in più (`↑N`, da pushare) |
| 🟣 viola  | hai commit in meno (`↓N`, da pullare) |
| 🔴 rosso  | divergenti (`↑N ↓M`) |
| ⚫ grigio | nessun upstream / HEAD detached / non ancora controllato |
| 🟠 ambra  | errore (cartella mancante, non è un repo git…) |

Altri indicatori: `*` ambra accanto al nome = modifiche non committate;
`⚠` = l'ultimo `git fetch` è fallito (lo stato mostrato si basa sugli ultimi ref remoti noti).
Passando col mouse su una riga si vedono path, branch → upstream, dettagli ed errori.

## Uso

```sh
cargo run --release                 # usa ./meerkat.txt
cargo run --release -- path/lista.txt -i 10
```

```
meerkat [LIST_FILE] [-i MINUTES]

  LIST_FILE        lista dei repo, un path per riga
                   (default: ./meerkat.txt; se è una directory: DIR/meerkat.txt)
  -i, --interval   minuti tra un fetch automatico e l'altro, 0 = disattivato (default: 5)
```

### Il file `meerkat.txt`

Un path locale per riga. Righe vuote e righe che iniziano con `#` sono ignorate;
i path relativi sono risolti rispetto alla cartella del file.

```
# lavoro
C:\src\progetto-a
/home/me/src/progetto-b
../altro-repo
```

Il file viene riscritto quando aggiungi, rimuovi o riordini repository dall'interfaccia
(in quel caso i commenti non vengono conservati).

### Interfaccia

- **⟳** (o `F5`): fetch di tutti i repo. Il contatore in alto a destra indica il prossimo fetch automatico.
- **+**: aggiunge un repo scrivendone il path; in alternativa trascina una o più cartelle sulla finestra.
- **☰**: intervallo di auto-fetch, "always on top", ricarica/apri il file della lista.
- Click destro su una riga: fetch singolo, apri cartella, copia path, sposta su/giù, rimuovi.
  Doppio click: apre la cartella.
- `Ctrl` `+` / `Ctrl` `-` / `Ctrl` `0`: zoom dell'interfaccia.

## Note tecniche

- Usa il comando `git` installato (deve essere nel `PATH`), quindi rispetta credential
  helper, agent SSH, proxy e configurazione dell'utente. Il fetch gira in background
  (max 6 repo in parallelo) con `GIT_TERMINAL_PROMPT=0`, così un repo che chiede la
  password non blocca nulla; ogni fetch ha un timeout di 90 s.
- L'upstream è quello configurato per il branch (`@{upstream}`); se manca si usa
  `origin/<branch>` quando esiste.
- Hi-DPI: la scala viene presa dal sistema (per-monitor su Windows, X11 e Wayland);
  tutti gli indicatori sono disegnati come grafica vettoriale.
- Renderer OpenGL (glow). Su Windows la build release non apre la console.

## Build

Serve una toolchain Rust stabile recente (edition 2024).

```sh
cargo build --release
```

Su Linux a runtime servono le librerie standard di un desktop (`libxkbcommon`,
`libGL`/`libEGL`, `libwayland` o `libX11`); su Ubuntu/Debian minimali:
`sudo apt install libxkbcommon-x11-0 libgl1 libegl1`.
