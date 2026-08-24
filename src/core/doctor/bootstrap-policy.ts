import { none, some, type Option } from "../types/option.js";

export interface BootstrapDenial {
  readonly block: true;
  readonly reason: "TIBER_BOOTSTRAP_READ_ONLY: repository mutation is unavailable until governed task workflows are installed";
}

const BLOCKED_MUTATION_TOOLS = new Set(["bash"]);

export function authorizeBootstrapTool(
  toolName: string,
): Option<BootstrapDenial> {
  if (!BLOCKED_MUTATION_TOOLS.has(toolName)) {
    return none;
  }

  return some({
    block: true,
    reason:
      "TIBER_BOOTSTRAP_READ_ONLY: repository mutation is unavailable until governed task workflows are installed",
  });
}
