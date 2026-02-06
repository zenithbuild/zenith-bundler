
import type { ZenManifest } from './types'

export function generateRuntime(manifest: ZenManifest, isDev: boolean = false): { code: string } {
    const routes = JSON.stringify(manifest.routes)

    return {
        code: `
import { createRouter } from '@zenithbuild/router'

// Initialize Router
const router = createRouter({
    routes: ${routes},
    isDev: ${isDev}
})

// Initialize Runtime
if (window.zenith) {
    window.zenith.router = router
    router.init()
}
`
    }
}
