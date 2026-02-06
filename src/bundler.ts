/**
 * @zenithbuild/bundler - Page Script Bundler
 * 
 * COMPILER-FIRST ARCHITECTURE
 * ═══════════════════════════════════════════════════════════════════════════════
 * 
 * This bundler performs ZERO inference. It executes exactly what the compiler specifies.
 * 
 * Rules:
 * - If a BundlePlan is provided, bundling MUST occur
 * - If bundling fails, throw a hard error (no fallback)
 * - The bundler never inspects source code for intent
 * - No temp files, no heuristics, no recovery
 * 
 * Bundler failure = compiler bug.
 * ═══════════════════════════════════════════════════════════════════════════════
 */

import { rolldown } from 'rolldown'
import type { BundlePlan } from '@zenithbuild/compiler'

/**
 * Execute a compiler-emitted BundlePlan
 * 
 * This is a PURE PLAN EXECUTOR. It does not:
 * - Inspect source code for imports
 * - Decide whether bundling is needed
 * - Fall back on failure
 * - Use temp files
 * 
 * @param plan - Compiler-emitted BundlePlan (must exist; caller must not call if no plan)
 * @throws Error if bundling fails (no fallback, no recovery)
 */
export async function bundlePageScript(plan: BundlePlan): Promise<string> {
    // Virtual entry module ID
    const VIRTUAL_ENTRY = '\0zenith:entry.tsx'

    // Build virtual modules map from plan
    const virtualModules = new Map<string, string>()
    virtualModules.set(VIRTUAL_ENTRY, plan.entry)
    for (const vm of plan.virtualModules) {
        virtualModules.set(vm.id, vm.code)
    }

    // Execute Rolldown with plan-specified configuration
    // No inference, no heuristics, no semantic analysis
    const bundle = await rolldown({
        input: VIRTUAL_ENTRY,
        platform: plan.platform,
        resolve: {
            modules: plan.resolveRoots
        },
        plugins: [{
            name: 'zenith-virtual',
            resolveId(source: string) {
                // Virtual modules from plan
                if (virtualModules.has(source)) {
                    return { id: source, moduleSideEffects: true }
                }
                // Special case: zenith:content namespace
                if (source === 'zenith:content') {
                    return { id: '\0zenith:content', moduleSideEffects: true }
                }
                return null
            },
            load(id: string) {
                return virtualModules.get(id) ?? null
            }
        }],
        // DETERMINISTIC OUTPUT: Disable all semantic optimizations
        // Tree-shaking implies semantic analysis - bundler must not infer
        treeshake: false,
        // HARD FAILURE on unresolved imports - no silent external treatment
        onLog(level: string, log: any) {
            if (log.code === 'UNRESOLVED_IMPORT') {
                throw new Error(
                    `[Zenith Bundler] Unresolved import: ${log.message}. ` +
                    `This is a compiler error - the BundlePlan references a module that cannot be resolved.`
                )
            }
        }
    })

    // Generate output with plan-specified format
    const { output } = await bundle.generate({
        format: plan.format
    })

    // Hard failure if no output - this is a compiler bug
    if (!output[0]?.code) {
        throw new Error(
            '[Zenith Bundler] Rolldown produced no output. ' +
            'This is a compiler error - the BundlePlan was invalid or Rolldown failed silently.'
        )
    }

    return output[0].code
}
