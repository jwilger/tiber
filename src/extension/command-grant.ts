import type { ExtensionCommandContext } from "@earendil-works/pi-coding-agent";

import { FileCommandAuthority } from "../adapters/commands/file-command-authority.js";

export async function handleCommandGrant(
  argumentsText: string,
  context: ExtensionCommandContext,
): Promise<void> {
  if (argumentsText.trim() !== "grant") {
    context.ui.notify("Usage: /tiber:commands grant", "info");
    return;
  }
  const authority = new FileCommandAuthority(context.cwd);
  const catalog = authority.loadCatalog();
  if (!catalog.ok) {
    context.ui.notify(
      `${catalog.failure.code}: ${catalog.failure.message}`,
      "error",
    );
    return;
  }
  if (!context.hasUI) {
    context.ui.notify(
      "TIBER_COMMAND_GRANT_HUMAN_REQUIRED: interactive confirmation required",
      "error",
    );
    return;
  }
  const phrase = `grant commands ${catalog.value.digest}`;
  const confirmation = await context.ui.input(
    "Grant exact project commands",
    `${catalog.value.canonicalJson}\n\nType: ${phrase}`,
  );
  if (confirmation !== phrase || !authority.grant(catalog.value.digest)) {
    context.ui.notify(
      "TIBER_COMMAND_GRANT_DENIED: exact grant was not persisted",
      "error",
    );
    return;
  }
  context.ui.notify(`Command catalog granted\n${catalog.value.digest}`, "info");
}
