# FluxDeck

Controlador ligero para Stream Deck en Linux, escrito en Rust. Son dos programas:

- `fluxdeck`: servicio que habla con el deck. Lee `~/.config/fluxdeck/config.toml`,
  aplica los cambios al guardar y se reconecta solo si desconectas el USB o suspendes.
- `fluxdeck-editor`: ventana para configurar teclas, perillas y páginas con vista previa.

Probado con el Stream Deck +. Los demás modelos que soporta
[elgato-streamdeck](https://crates.io/crates/elgato-streamdeck) deberían funcionar con el
servicio; el editor por ahora dibuja la distribución del +.

## Qué hace

- Teclas: un comando, abrir otra página, o ambas cosas.
- Perillas: un comando al girar a cada lado y otro al presionar o tocar la tira.
  `$FLUXDECK_STEPS` dice cuántos pasos giró. Trae plantillas de volumen y micrófono (PipeWire).
- Estado en vivo: un comando que se repite cada N segundos; su primera línea reemplaza el
  texto. Puede imprimir `texto|nivel|#color`: `42 dB|0.5` dibuja una barra y
  `Mute||#b3261e` pinta el fondo de la tecla.
- Medidor: una perilla puede mostrar el nivel de entrada en vivo de una fuente de PipeWire.
- Biblioteca de acciones en el editor (sistema, audio, multimedia, Hyprland, XLR Dock, filtros
  RNNoise). Solo aparece lo que existe en tu equipo.
- Deslizar la tira táctil cambia de página.
- Iconos: cualquier glifo de una Nerd Font (el editor los busca por nombre) o una imagen PNG, JPEG o SVG.
- Plugins de Elgato y OpenDeck, con tienda integrada (ver abajo).
- Módulos opcionales por marca de periféricos: hoy, Corsair.

## Instalación

Necesitas Rust, `fontconfig` y `libudev` (systemd).

```sh
dist/install.sh          # binarios en ~/.local/bin, servicio de usuario y lanzador
dist/install.sh --udev   # además, la regla udev (pide tu contraseña)
```

La regla udev hace falta si tu usuario no puede abrir el deck (el servicio lo dice en
`journalctl --user -u fluxdeck`) y para 48 V y Baja Z del XLR Dock. Si el servicio no está
corriendo, el editor lo avisa arriba a la derecha y lo puede iniciar.

Atajos del editor: Ctrl+S guarda, Ctrl+Z / Ctrl+Shift+Z deshace y rehace, Ctrl+C / Ctrl+V / Ctrl+D
copian, pegan y duplican la tecla seleccionada, Supr la quita, Esc suelta la selección y Ctrl+1…4
cambian de vista. Clic derecho en una tecla o perilla abre las mismas acciones.

## Elgato XLR Dock

La ganancia, el mute y los audífonos son controles ALSA normales (`amixer -c Dock`). El phantom
48 V y la baja impedancia van por la interfaz USB del fabricante:

```sh
fluxdeck xlr phantom            # on / off
fluxdeck xlr phantom toggle
fluxdeck xlr lowz on
```

Necesita la regla udev de `dist/`. El formato del bloque viene del trabajo de
[OpenWave](https://github.com/rikkichy/openwave) y [OpenXLR](https://github.com/Michael-Brooks/openxlr);
no los uses al mismo tiempo que FluxDeck.

## Plugins

FluxDeck corre plugins hechos para el software de Elgato y para OpenDeck (el mismo protocolo).
En el editor, la pestaña **Plugins** es una tienda con dos catálogos:

- **Plugins libres**: el catálogo de [OpenAction](https://openactionapi.github.io/plugins/).
  Los marcados «Hecho para Linux» corren tal cual.
- **Tienda de Elgato**: un archivo de la tienda oficial. Casi todos son para Windows y necesitan
  Wine; algunos no funcionan.

Al instalar uno, sus acciones aparecen en la biblioteca. Los ajustes de cada acción se abren en
el navegador desde el inspector del editor y se guardan en `~/.config/fluxdeck/plugin-settings.json`.
Los plugins de Node.js necesitan `node` 20 o más nuevo; los de Windows, `wine`. Los plugins se
instalan en `~/.local/share/fluxdeck/plugins` y su registro queda en
`~/.local/state/fluxdeck/plugins`. `FLUXDECK_DEBUG=1` muestra cada mensaje con los plugins.

```sh
fluxdeck plugin search linux
fluxdeck plugin install me.amankhanna.oampris
fluxdeck plugin install --elgato com.ejemplo.plugin
fluxdeck plugin list
fluxdeck plugin remove me.amankhanna.oampris
```

Pendiente: los plugins que dibujan la tira con diseños propios (`setFeedbackLayout`) se ven con
el diseño de FluxDeck (título, valor y barra), y los plugins para otros aparatos (Ajazz, Mirabox…)
no aplican.

## Periféricos Corsair (opcional)

Se activa en el editor, en *Módulos*, o con `modules = ["corsair"]` en la config. Usa
[OpenLinkHub](https://github.com/jurkovic-nikola/OpenLinkHub), que debe estar instalado y
corriendo. Agrega la vista **Periféricos** (batería, cancelación de ruido, brillo y efecto de luz
de cada aparato) y acciones para el deck. Desde la terminal:

```sh
fluxdeck corsair list
fluxdeck corsair battery SERIE
fluxdeck corsair anc SERIE cycle
fluxdeck corsair brightness SERIE up
fluxdeck corsair effect SERIE next
```

## Efectos del micrófono

En la vista **Micrófono**, «Editar efectos» crea una cadena propia del servicio a partir del filtro
que ya tengas (o una vacía). Se guarda en `~/.config/fluxdeck/effects.toml` y el servicio la corre
como un micrófono más, «Micrófono con efectos», sin tocar la config de PipeWire del sistema.

- Efectos de PipeWire (ecualizadores), LADSPA y LV2: todos los que estén instalados.
  `fluxdeck effects scan` los lista.
- Los valores cambian al momento; agregar, quitar, mover o apagar un efecto reinicia la cadena y
  corta el micrófono un instante.
- Sirve con cualquier micrófono: «Micrófono de entrada» elige cuál procesar (USB, audífonos,
  el integrado, una interfaz de audio…).
- El selector «Micrófono que usan las apps» elige entre el micrófono directo, el filtro del sistema
  y esta cadena.
- «Paquetes de efectos», al final de la lista de agregar, instala LSP, x42, Calf, SWH o RNNoise con
  el gestor de paquetes del sistema (pacman, apt, dnf o zypper) en una terminal, donde escribes tu
  contraseña. Los nombres de paquete de pacman están comprobados; los de las demás distros, no.
- `fluxdeck effects import` crea la cadena desde la terminal.

Los plugins VST no se cargan: en Linux, LV2 y LADSPA cumplen ese papel y PipeWire los corre sin
programas en medio.

## PipeWire

```sh
fluxdeck pw param capture.mi_filtro 'rnnoise:VAD Threshold (%)' +5 0 99
fluxdeck pw default                       # fuente predeterminada
fluxdeck pw toggle-default fuente_a fuente_b
```

Los cambios de `param` duran hasta que se reinicia el filtro; el archivo del filtro no se toca.

## Colores

Los colores salen de `[theme]` en la config. Con `palette = "ruta/colors.json"` siguen en vivo una
paleta Material You (la que escribe matugen; en Ryoku, `~/.cache/ryoku/colors.json`). Si existe
`~/.config/fluxdeck/theme.toml`, manda sobre todo lo anterior:

```toml
background = "#101014"
key = "#1d1d24"
text = "#e8e6ee"
accent = "#9b8cff"
```
