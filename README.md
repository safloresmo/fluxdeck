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

Es modular: en *Ajustes → Extensiones → Qué usas* (y la primera vez que lo abres) eliges qué
partes quieres: deck, deck en el celular, micrófono, mezclador y grupos, periféricos, perfiles y
plugins. Lo que apagas desaparece del editor y no corre en el servicio.

**Deck**
- Teclas y perillas con comandos, páginas, estados en vivo (`texto|nivel|#color`) y medidores.
- Cada tecla puede hacer otra cosa al mantenerla pulsada medio segundo.
- Biblioteca de acciones: sistema, audio, multimedia, Hyprland, XLR Dock, efectos, Corsair.
  Solo aparece lo que existe en tu equipo.
- Arrastrar y soltar, copiar y pegar, deshacer, menú de clic derecho, duplicar páginas.
- Plugins de Elgato y OpenDeck, con tienda integrada.
- Tienda de íconos: más de 200 000 íconos libres de Iconify (Material, logos de apps, emojis…).

**Audio**
- Mezclador en cuadrícula, como Wave Link: una fila por canal (micrófono, grupos y apps) y una
  columna por salida; un clic en el cruce manda el canal a esa salida.
- Mezcla para stream opcional: una salida «FluxDeck Stream» para OBS, con volumen y silencio
  propios para el micrófono y cada grupo, independientes de lo que oyes. Solo existe mientras
  está encendida; una app sin grupo se agrupa con un clic para mandarla.
- Grupos de apps (Juegos, Música, Navegador, Chat…): cada grupo tiene su volumen y las apps
  nuevas entran solas. Con la mezcla apagada no crea aparatos virtuales.
- Perillas del deck para el volumen de una app o de un grupo.
- Micrófono: cadena de efectos propia (LADSPA, LV2 y los de PipeWire), con instalador de
  paquetes de efectos, y controles del Elgato XLR Dock.

**Periféricos y más**
- Módulo Corsair (a través de OpenLinkHub): batería, cancelación de ruido y luces.
- Otras marcas: luces con OpenRGB, auriculares con HeadsetControl y baterías inalámbricas con UPower.
- Inicio con el estado de todo, avisos y controles rápidos del micrófono y los grupos; tema del
  escritorio, oscuro, claro o propio; español e inglés.
- Perfiles que cambian página, volúmenes o un comando según la app que usas; se crean desde una
  ventana usada hace poco. Siguen la ventana activa en Hyprland, Sway, i3, niri, KDE Plasma,
  escritorios X11 (XFCE, Cinnamon, MATE…) y GNOME con la extensión
  [Focused Window D-Bus](https://extensions.gnome.org/extension/5592/focused-window-d-bus/).
- El deck se apaga al bloquear la sesión o suspender, y si quieres tras un rato sin tocarlo.
- Respaldo de toda la configuración en un archivo.
- Ícono en la bandeja; al arrancar con la sesión no abre ninguna ventana.
- En Ryoku, instala y muestra desde los ajustes un widget para la barra, que se distribuye aparte.

## Instalación

Funciona en cualquier distribución con glibc 2.35 o más nueva (Ubuntu 22.04+, Debian 12+,
Fedora 36+, Arch, openSUSE Tumbleweed…). Necesitas `fontconfig`, `libudev` (systemd) y PipeWire
con `wpctl`, `pw-dump` y `pw-record`: el volumen, el mezclador y los efectos no funcionan con
PulseAudio solo (Ubuntu 22.04 lo trae de serie; desde 22.10 ya viene PipeWire).

En [Releases](https://github.com/safloresmo/fluxdeck/releases) (Linux x86_64):

- **Ubuntu, Debian y derivadas**: descarga el `.deb` y ábrelo, o `sudo apt install ./fluxdeck_<versión>_amd64.deb`.
- **Fedora**: `sudo dnf install ./fluxdeck-<versión>-1.x86_64.rpm`.
- **openSUSE**: `sudo zypper install ./fluxdeck-<versión>-1.x86_64.rpm`.

Los paquetes traen la regla udev, así que el deck funciona sin más pasos. Luego abre FluxDeck desde
el menú de apps: la primera vez pregunta qué quieres usar, y desde Inicio se inicia el servicio.

En cualquier otra distribución, descarga `fluxdeck-<versión>-x86_64-linux.tar.gz`, descomprímelo
y, dentro de su carpeta:

```sh
dist/install.sh          # binarios en ~/.local/bin, servicio de usuario, lanzador e ícono
dist/install.sh --udev   # además, la regla udev (pide tu contraseña)
```

Desde el código hace falta además Rust; `dist/install.sh` compila antes de instalar.

La regla udev hace falta si tu usuario no puede abrir el deck (el servicio lo dice en
`journalctl --user -u fluxdeck`) y para 48 V y Baja Z del XLR Dock. Si el servicio no está
corriendo, el editor lo avisa y lo puede iniciar.

Cómo arranca se elige en *Ajustes → General → Arranque*:

- **Servicio del sistema**: el servicio de usuario de systemd (lo que deja la primera instalación).
- **Lo arranca mi escritorio**: escribe `~/.config/autostart/fluxdeck.desktop`, que siguen GNOME,
  KDE, XFCE y la mayoría. En Ryoku se agrega en *Ryoku Hub → Sesión → Al iniciar*.
- **No arrancar solo**.

En los tres casos arranca minimizado: solo el ícono de la bandeja. Las actualizaciones con
`dist/install.sh` respetan lo que elegiste.

Los cambios se guardan solos al terminar cada uno; arriba a la derecha dice «Guardado ✓».

Atajos del editor: Ctrl+S guarda al momento, Ctrl+Z / Ctrl+Shift+Z deshace y rehace, Ctrl+C / Ctrl+V / Ctrl+D
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
fluxdeck mix mic toggle           # el micrófono en uso
fluxdeck mix status               # todo en una línea JSON (lo usa el widget de Ryoku)
```

En la cuadrícula del mezclador, el punto de cada grupo marca su salida. El grupo la recuerda
(`~/.local/state/fluxdeck/group-outputs.json`) y manda ahí también las apps que se abran después.

## Mezcla para stream

El botón *Mezcla para stream* (en *Audio → Mezclador*) crea la salida «FluxDeck Stream» y una
columna Stream en la cuadrícula. El micrófono y cada grupo tienen ahí su envío, con volumen
propio; todos empiezan apagados y se encienden con su botón o moviendo su barra. En OBS se agrega
una captura de audio de salida con «FluxDeck Stream».

Lo que oyes y lo que va al stream son independientes, como en Wave Link: mientras la mezcla está
encendida, cada grupo tiene su propia salida («FluxDeck · Juegos», «FluxDeck · Chat»…) que sigue a
tu salida con el volumen y el silencio del grupo, y el envío al stream toma el sonido antes de ese
volumen. Silenciar Música para ti no la silencia en el stream, y al revés.

Esas salidas y «FluxDeck Stream» las mantiene el servicio: desaparecen al apagar la mezcla o al
cerrarse FluxDeck, y las apps vuelven a su salida de siempre. Una app tiene que estar en un grupo
para llegar al stream.

```sh
fluxdeck mix stream on            # on / off / toggle
```

## Ryoku

En un escritorio Ryoku, *Ajustes → Extensiones → Integraciones* instala el widget de FluxDeck
para la barra (el micrófono, los grupos, el perfil y la mezcla para stream), lo muestra u oculta,
lo actualiza cuando FluxDeck trae una versión nueva y lo quita. El widget no está en este
repositorio: `dist/install.sh` lo copia si su carpeta `fluxdeck-widget` está junto a esta.
FluxDeck solo usa los comandos de Ryoku (`ryoku plugin`, `ryoku-shell bar`).

## Elgato XLR Dock

La ganancia, el mute y los audífonos son controles ALSA normales (`amixer -c Dock`). El phantom
48 V y la baja impedancia van por la interfaz USB del fabricante:

```sh
fluxdeck xlr phantom            # on / off
fluxdeck xlr phantom toggle
fluxdeck xlr lowz on
```

Necesita la regla udev de `dist/`. En el editor, encender los 48 V pide un segundo clic: solo
los micrófonos de condensador los necesitan. El formato del bloque viene del trabajo de
[OpenWave](https://github.com/rikkichy/openwave) y [OpenXLR](https://github.com/Michael-Brooks/openxlr);
no los uses al mismo tiempo que FluxDeck.

## Plugins

FluxDeck corre plugins hechos para el software de Elgato y para OpenDeck (el mismo protocolo).
En el editor, la pestaña **Plugins** es una tienda con dos catálogos:

- **Plugins libres**: el catálogo de [OpenAction](https://openactionapi.github.io/plugins/).
  Los marcados «Hecho para Linux» corren tal cual.
- **Tienda de Elgato**: un archivo de la tienda oficial. Casi todos son para Windows y necesitan
  Wine; algunos no funcionan.

Al instalar uno, sus acciones aparecen en la biblioteca. Cada plugin instalado se enciende, se
apaga o se quita desde esa misma pestaña. Los ajustes de cada acción se abren en
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

## Deck en el celular

*Ajustes → Extensiones → Deck en el celular* convierte un celular o una tablet en tu misma red en
un Stream Deck: muestra tus teclas, páginas y perillas con las mismas imágenes y estados, y cada
toque hace lo mismo que la tecla. Funciona como copia de tu Stream Deck o sin ninguno conectado (con
la forma de un Stream Deck +). Se abre escaneando el QR; en el navegador, «Añadir a pantalla de
inicio» lo deja como una app.

Está apagado hasta que lo enciendes. El servidor solo responde a redes locales y privadas, y cada
pedido necesita la clave secreta que lleva el QR: quien la tenga puede usar tus teclas, así que no
la compartas, y si lo hiciste, *Cambiar clave* deja fuera a los celulares que la tenían. Usa el
puerto 27080 (o el siguiente libre); si tienes un firewall, ábrelo para tu red local.

## Respaldo

*Ajustes → General → Respaldo* guarda todo en un archivo `fluxdeck-<fecha>.tar.gz` dentro de
`~/Documentos/FluxDeck`: páginas, teclas, perillas, grupos y sus salidas, perfiles, tema, efectos del
micrófono, la mezcla para stream y los íconos de la tienda. Los ajustes de los plugins solo van si
los marcas, porque pueden tener claves de acceso. Los plugins no van: el respaldo guarda su lista y,
al importarlo, dice cuáles faltan.

Importar reemplaza la configuración, pero antes guarda una copia de la actual en
`~/.local/share/fluxdeck/respaldos/`. Las rutas de los íconos se ajustan solas a la carpeta del
equipo nuevo. «Otro archivo…» usa `zenity` para elegir un archivo fuera de la carpeta.

```sh
fluxdeck backup export                     # --plugin-settings para incluir sus ajustes
fluxdeck backup list
fluxdeck backup import ~/Documentos/FluxDeck/fluxdeck-2026-10-25-181830.tar.gz
```

## Íconos

Junto al campo «Icono» de una tecla o perilla, **Tienda de íconos…** abre los packs libres de
[Iconify](https://iconify.design): Material Design, Lucide, Fluent, logos de marcas, emojis y más.
Se abre buscando el nombre de la tecla (los nombres en español comunes se pasan al inglés, que es
como Iconify nombra sus íconos), y sin búsqueda muestra los packs destacados. Cada pack dice su
licencia.

El ícono elegido se guarda como SVG en `~/.local/share/fluxdeck/icons/`, así que la tecla no
necesita internet. Los íconos de un solo color toman el color del texto del tema; los de color
conservan los suyos.

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

## Periféricos de otras marcas (opcional)

FluxDeck no habla con cada marca por su cuenta: usa los programas de Linux que ya las conocen.
Cada uno agrega sus tarjetas a la vista **Periféricos** y sus acciones a la biblioteca.
Si hay conectado un aparato cuyo programa falta o está apagado, **Periféricos** lo dice y ofrece
abrir una terminal con el comando listo (se ejecuta al pulsar Enter), abrir el programa o activar
el módulo.

- **Luces** (Logitech, Razer, SteelSeries, ASUS, MSI, Gigabyte, memorias RAM y más): con
  [OpenRGB](https://openrgb.org) abierto y su servidor SDK encendido (pestaña *SDK Server*, o
  `openrgb --server`). Se activa en *Módulos* o con `"openrgb"` en `modules`. Efecto, color y brillo.
- **Auriculares** (Logitech, SteelSeries, HyperX, Corsair y
  [otros](https://github.com/Sapd/HeadsetControl#supported-headsets)): con
  [HeadsetControl](https://github.com/Sapd/HeadsetControl) instalado. Se activa en *Módulos* o con
  `"headset"` en `modules`. Batería, retorno de voz y luces.
- **Baterías** de ratones, teclados y controles inalámbricos: siempre, si UPower las ve (los
  Logitech con receptor Unifying o Lightspeed y lo conectado por Bluetooth, por ejemplo).

```sh
fluxdeck rgb list
fluxdeck rgb effect 'NOMBRE' next
fluxdeck rgb color 'NOMBRE' ff8800
fluxdeck rgb brightness 'NOMBRE' up
fluxdeck headset list
fluxdeck headset sidetone VENDOR:PRODUCT up
fluxdeck headset lights VENDOR:PRODUCT toggle
fluxdeck battery list
fluxdeck battery 'MODELO'
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
