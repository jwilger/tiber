import {
  getAgentDir,
  type ExtensionCommandContext,
} from "@earendil-works/pi-coding-agent";

import { FileAuthorityStore } from "../adapters/settings/file-authority-store.js";
import { FileSettingsStore } from "../adapters/settings/file-settings-store.js";
import {
  parseSettingsCommand,
  type SettingsScope,
} from "../core/configuration/settings-command.js";
import {
  formatAuthority,
  lockMinimumAssurance,
  setSecretReference,
  unlockMinimumAssurance,
} from "../core/configuration/authority.js";
import {
  ASSURANCE_LEVELS,
  formatSettingsTable,
  resolveSettings,
  setSetting,
  WORKTREE_MODES,
  type SettingKey,
} from "../core/configuration/settings.js";

function notifyFailure(
  context: ExtensionCommandContext,
  code: string,
  message: string,
): void {
  context.ui.notify(`${code}: ${message}`, "error");
}

async function selectValue(
  context: ExtensionCommandContext,
  key: SettingKey,
): Promise<string | undefined> {
  if (key === "assuranceLevel") {
    return context.ui.select("Assurance level", [
      "inherit",
      ...ASSURANCE_LEVELS,
    ]);
  }
  if (key === "worktreeMode") {
    return context.ui.select("Worktree mode", ["inherit", ...WORKTREE_MODES]);
  }

  const entered = await context.ui.input(
    "Output preview bytes",
    "Empty means inherit; valid range is 1024 to 1048576",
  );
  return entered === undefined ? undefined : entered.trim() || "inherit";
}

async function editInteractively(
  context: ExtensionCommandContext,
  store: FileSettingsStore,
): Promise<void> {
  const loaded = store.load();
  if (!loaded.ok) {
    notifyFailure(context, loaded.failure.code, loaded.failure.message);
    return;
  }

  const table = formatSettingsTable(
    loaded.value.globalValues,
    loaded.value.projectValues,
  );
  const scopeChoice = await context.ui.select(`Tiber settings\n\n${table}`, [
    "Close",
    "Edit user global",
    "Edit project",
  ]);
  if (scopeChoice === undefined || scopeChoice === "Close") {
    return;
  }

  const scope: SettingsScope =
    scopeChoice === "Edit user global" ? "global" : "project";
  const keyChoice = await context.ui.select("Setting", [
    "assuranceLevel",
    "outputPreviewBytes",
    "worktreeMode",
  ]);
  if (
    keyChoice !== "assuranceLevel" &&
    keyChoice !== "outputPreviewBytes" &&
    keyChoice !== "worktreeMode"
  ) {
    return;
  }

  const value = await selectValue(context, keyChoice);
  if (value === undefined) {
    return;
  }

  const current =
    scope === "global" ? loaded.value.globalValues : loaded.value.projectValues;
  const updated = setSetting(current, keyChoice, value);
  if (!updated.ok) {
    notifyFailure(context, updated.failure.code, updated.failure.message);
    return;
  }

  const written =
    scope === "global"
      ? store.saveGlobal(updated.value)
      : store.saveProject(loaded.value.projectId, updated.value);
  if (!written.ok) {
    notifyFailure(context, written.failure.code, written.failure.message);
    return;
  }

  const refreshed = store.load();
  if (!refreshed.ok) {
    notifyFailure(context, refreshed.failure.code, refreshed.failure.message);
    return;
  }
  context.ui.notify(
    formatSettingsTable(
      refreshed.value.globalValues,
      refreshed.value.projectValues,
    ),
    "info",
  );
}

export async function handleSettingsCommand(
  argumentsText: string,
  context: ExtensionCommandContext,
): Promise<void> {
  const agentDirectory = getAgentDir();
  const store = new FileSettingsStore(agentDirectory, context.cwd);
  const authorityStore = new FileAuthorityStore(agentDirectory);
  if (argumentsText.trim().length === 0 && context.mode === "tui") {
    await editInteractively(context, store);
    return;
  }

  const command = parseSettingsCommand(argumentsText);
  if (!command.ok) {
    notifyFailure(context, command.failure.code, command.failure.message);
    return;
  }

  const loaded = store.load();
  if (!loaded.ok) {
    notifyFailure(context, loaded.failure.code, loaded.failure.message);
    return;
  }

  const authority = authorityStore.load();
  if (!authority.ok) {
    notifyFailure(context, authority.failure.code, authority.failure.message);
    return;
  }

  if (command.value.kind === "set") {
    const current =
      command.value.scope === "global"
        ? loaded.value.globalValues
        : loaded.value.projectValues;
    const updated = setSetting(current, command.value.key, command.value.value);
    if (!updated.ok) {
      notifyFailure(context, updated.failure.code, updated.failure.message);
      return;
    }

    const written =
      command.value.scope === "global"
        ? store.saveGlobal(updated.value)
        : store.saveProject(loaded.value.projectId, updated.value);
    if (!written.ok) {
      notifyFailure(context, written.failure.code, written.failure.message);
      return;
    }
  } else if (command.value.kind === "lock") {
    const lockValue = command.value.value;
    const level = ASSURANCE_LEVELS.find((candidate) => candidate === lockValue);
    if (level === undefined) {
      notifyFailure(
        context,
        "TIBER_SETTINGS_INVALID_VALUE",
        `invalid assurance lock: ${lockValue}`,
      );
      return;
    }
    const saved = authorityStore.save(
      lockMinimumAssurance(authority.value, level),
    );
    if (!saved.ok) {
      notifyFailure(context, saved.failure.code, saved.failure.message);
      return;
    }
  } else if (command.value.kind === "unlock") {
    const unlocked = unlockMinimumAssurance(
      authority.value,
      command.value.confirmation,
    );
    if (!unlocked.ok) {
      notifyFailure(context, unlocked.failure.code, unlocked.failure.message);
      return;
    }
    const saved = authorityStore.save(unlocked.value);
    if (!saved.ok) {
      notifyFailure(context, saved.failure.code, saved.failure.message);
      return;
    }
  } else if (command.value.kind === "secret") {
    const updated = setSecretReference(
      authority.value,
      command.value.key,
      command.value.environmentName,
    );
    if (!updated.ok) {
      notifyFailure(context, updated.failure.code, updated.failure.message);
      return;
    }
    const saved = authorityStore.save(updated.value);
    if (!saved.ok) {
      notifyFailure(context, saved.failure.code, saved.failure.message);
      return;
    }
  }

  const refreshed = store.load();
  if (!refreshed.ok) {
    notifyFailure(context, refreshed.failure.code, refreshed.failure.message);
    return;
  }
  const refreshedAuthority = authorityStore.load();
  if (!refreshedAuthority.ok) {
    notifyFailure(
      context,
      refreshedAuthority.failure.code,
      refreshedAuthority.failure.message,
    );
    return;
  }
  const requested = resolveSettings(
    refreshed.value.globalValues,
    refreshed.value.projectValues,
  ).assuranceLevel.value;
  context.ui.notify(
    `${formatSettingsTable(
      refreshed.value.globalValues,
      refreshed.value.projectValues,
    )}\n\n${formatAuthority(refreshedAuthority.value, requested)}`,
    "info",
  );
}
