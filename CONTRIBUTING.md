# Contribuir a Red TUI

Gracias por contribuir. Antes de comenzar, abre un issue para describir cambios
grandes o nuevas funciones.

## Flujo recomendado

1. Crea una rama desde `main`.
2. Mantén los cambios pequeños y enfocados.
3. Agrega o actualiza pruebas cuando cambie el comportamiento.
4. Verifica el proyecto antes de enviar el pull request:

```bash
cargo fmt -- --check
cargo test
cargo clippy -- -D warnings
```

## Pull requests

Incluye una descripción clara, cómo probaste el cambio y cualquier diferencia
relevante entre macOS y Linux.

No incluyas capturas, direcciones IP privadas, direcciones MAC, credenciales ni
resultados obtenidos de redes sin autorización.
