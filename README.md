# Red TUI

Monitor de red interactivo para terminal, desarrollado con Rust, Ratatui y
Crossterm. Permite revisar la conectividad local y hacia Internet, ejecutar
multiping y descubrir dispositivos de la red.

> Proyecto en desarrollo. Ejecuta escaneos solamente sobre redes propias o
> autorizadas.

## Funciones

- Pantalla de bienvenida para seleccionar el modulo.
- Deteccion automatica del gateway y la interfaz de red.
- Estado general de conectividad local e Internet.
- Multiping concurrente a direcciones IP o dominios.
- Estado individual, latencia, perdida de paquetes e historial grafico.
- Escaneo activo de la red local y lectura de vecinos ARP.
- Representacion de la topologia descubierta en la terminal.
- Compatibilidad con macOS y Linux.

## Requisitos

- Rust y Cargo.
- Comandos del sistema `ping`, `arp` y `traceroute`.
- Una terminal compatible con colores y caracteres Unicode.

Comprueba la instalacion de Rust:

```bash
rustc --version
cargo --version
```

## Ejecutar

```bash
git clone <URL-DEL-REPOSITORIO>
cd red-tui
cargo run --release
```

Tambien puedes instalar el binario localmente:

```bash
cargo install --path .
red-tui
```

## Modulos

### Monitor de red

Muestra la interfaz utilizada, el gateway detectado, el estado de los destinos
vigilados y la cantidad de vecinos encontrados.

### Pings multiples

Ejecuta pings concurrentes cada dos segundos. Para cada destino muestra:

- Estado en linea o sin respuesta.
- Ultima latencia registrada.
- Porcentaje de perdida de paquetes.
- Grafico con el historial reciente de latencia.

### Grafico de red

Realiza un escaneo de la red `/24` correspondiente al gateway, combina
respuestas de ping con vecinos ARP y muestra que dispositivos fueron
alcanzados. Tambien ejecuta un trace hacia `1.1.1.1` para visualizar los saltos
de salida a Internet.

## Controles

| Tecla | Accion |
| --- | --- |
| `1` | Abrir monitor de red |
| `2` | Abrir pings multiples |
| `3` | Abrir grafico de red |
| `↑` / `↓` | Cambiar seleccion; en topologia selecciona dispositivos |
| `PgUp` / `PgDn` | Desplazar los saltos del trace en topologia |
| `Home` / `End` | Ir al primer o ultimo dispositivo/salto en topologia |
| `Enter` | Abrir opcion o confirmar destino |
| `a` | Agregar IP o dominio al multiping |
| `d` | Eliminar destino seleccionado |
| `s` | Escanear dispositivos de la red y tracear `1.1.1.1` |
| `Esc` | Volver a la bienvenida o cancelar |
| `q` | Salir |

## Limitaciones

La vista de topologia usa una representacion isometrica portable de Ratatui.
Los widgets 3D de Ratty requieren un terminal compatible con Ratty Graphics
Protocol, por lo que no se habilitan por defecto.

El programa utiliza `ping`, `arp` y la tabla de rutas del sistema. ARP solo
permite descubrir dispositivos dentro de la red local y no identifica toda la
ruta fisica.

Los switches de capa 2 no aparecen en ARP ni traceroute. Para mostrar switches,
VLAN, puertos y conexiones fisicas con exactitud seria necesario integrar
SNMP, LLDP o la API de los equipos de red.

El escaneo activo envia pings a las direcciones de la red local `/24`.
Utilizalo solamente en redes propias o autorizadas.

## Desarrollo

```bash
cargo fmt -- --check
cargo test
cargo clippy -- -D warnings
```

Las contribuciones son bienvenidas. Consulta [CONTRIBUTING.md](CONTRIBUTING.md)
antes de abrir un pull request.

## Seguridad

Para reportar una vulnerabilidad, consulta [SECURITY.md](SECURITY.md). No
publiques credenciales, direcciones privadas sensibles ni resultados de redes
que no administras.

## Licencia

Distribuido bajo la licencia MIT. Consulta [LICENSE](LICENSE).
