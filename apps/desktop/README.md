# MyCodex Desktop

Desktop client shell for the APP surface.

## Stack

- Tauri
- React
- TypeScript
- TanStack Query
- Zustand

## Development

```bash
cd apps/desktop
npm install
npm run tauri:dev
```

The desktop shell is expected to talk to the daemon over:

- `POST /api/app/pairings/request`
- `GET /api/app/pairings/{pairing_id}`
- `GET /ws` with a bearer token
