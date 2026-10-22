<p align="center">
  <img src="assets/fluxdeck.svg" width="128" alt="FluxDeck">
</p>

<h1 align="center">FluxDeck</h1>

<p align="center">Tu Stream Deck, tu audio y tus periféricos, en una sola app ligera para Linux.</p>

FluxDeck junta en un programa lo que normalmente piden tres: el software del Stream Deck, un
mezclador de audio al estilo Wave Link y el control de periféricos. Está escrito en Rust, sigue
los colores de tu escritorio y el servicio gasta unos 15 MB de memoria.

Son dos programas:

- `fluxdeck`: el servicio. Habla con el deck, corre los plugins, la cadena de efectos del
  micrófono y los grupos de apps. Arranca con tu sesión.
- `fluxdeck-editor`: la ventana para configurarlo todo.

Probado con un Stream Deck +, un Elgato XLR Dock y periféricos Corsair en Arch Linux con
Hyprland y PipeWire. Los demás modelos que soporta
[elgato-streamdeck](https://crates.io/crates/elgato-streamdeck) deberían funcionar con el
servicio; el editor por ahora dibuja la distribución del +.

## Qué hace

**Deck**
- Teclas y perillas con comandos, páginas, estados en vivo (`texto|nivel|#color`) y medidores.
- Biblioteca de acciones: sistema, audio, multimedia, Hyprland, XLR Dock, efectos, Corsair.
  Solo aparece lo que existe en tu equipo.
- Arrastrar y soltar, copiar y pegar, deshacer, menú de clic derecho, duplicar páginas.
- Plugins de Elgato y OpenDeck, con tienda integrada.

**Audio**
- Mezclador: salida y micrófono en uso, y cada app que suena o graba, con su volumen, silencio,
  medidor y salida propia.
- Grupos de apps (Juegos, Música, Navegador, Chat…): cada grupo tiene su volumen y las apps
  nuevas entran solas. Sin aparatos virtuales: no ensucia los ajustes de sonido del sistema.
- Perillas del deck para el volumen de una app o de un grupo.
- Micrófono: cadena de efectos propia (LADSPA, LV2 y los de PipeWire), con instalador de
  paquetes de efectos, y controles del Elgato XLR Dock.

**Periféricos y más**
- Módulo Corsair (a través de OpenLinkHub): batería, cancelación de ruido y luces.
- Inicio con el estado de todo y avisos; tema del escritorio, oscuro, claro o propio; español
  e inglés.

## Instalación

Necesitas Rust, `fontconfig`, `libudev` (systemd), PipeWire con `wpctl`, `pw-dump` y `pw-record`.

```sh
dist/install.sh          # binarios en ~/.local/bin, servicio de usuario, lanzador e ícono
dist/install.sh --udev   # además, la regla udev (pide tu contraseña)
```

La regla udev hace falta si tu usuario no puede abrir el deck (el servicio lo dice en
`journalctl --user -u fluxdeck`) y para 48 V y Baja Z del XLR Dock. Si el servicio no está
corriendo, el editor lo avisa y lo puede iniciar.

Atajos del editor: Ctrl+S guarda, Ctrl+Z / Ctrl+Shift+Z deshace y rehace, Ctrl+C / Ctrl+V / Ctrl+D
copian, pegan y duplican la tecla seleccionada, Supr la quita, Esc suelta la selección, Ctrl+1…6
cambian de sección y Ctrl+, abre los ajustes. Clic derecho en una tecla o perilla abre las mismas
acciones.

## Grupos de apps

En *Audio → Grupos* cada grupo lleva una lista de nombres de apps (basta una parte: «chrom»
sirve para Chrome y Chromium). El servicio aplica el volumen y el silencio del grupo a esas apps,
también a las que se abran después. Los niveles se guardan en `~/.local/state/fluxdeck/groups.json`.

```sh
fluxdeck mix group musica +10     # sube el grupo Música
fluxdeck mix group chat toggle    # silencia o devuelve el sonido
fluxdeck mix app spotify =40      # una app suelta
```

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
