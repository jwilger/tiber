export interface BootstrapDenial {
  readonly block: true;
  readonly reason: "TIBER_BOOTSTRAP_READ_ONLY: repository mutation is unavailable until governed task workflows are installed";
}

const BLOCKED_MUTATION_TOOLS = new Set(["bash"]);

export function authorizeBootstrapTool(
  toolName: string,
): BootstrapDenial | undefined {
  if (!BLOCKED_MUTATION_TOOLS.has(toolName)) {
    return undefined;
  }

  return {
    block: true,
    reason:
      "TIBER_BOOTSTRAP_READ_ONLY: repository mutation is unavailable until governed task workflows are installed",
  };
}
