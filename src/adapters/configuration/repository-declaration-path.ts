import { existsSync, mkdirSync, realpathSync } from "node:fs";
import { isAbsolute, join, relative } from "node:path";

import {
  operationalFailure,
  type TiberFailure,
  type TiberResult,
} from "../../core/failures/tiber-failure.js";
import { none, some, type Option } from "../../core/types/option.js";

declare const declarationPathPurpose: unique symbol;
export type CanonicalRepositoryDeclarationPath = string & {
  readonly [declarationPathPurpose]: "canonical-repository-declaration-path";
};

type RepositoryDeclarationPathFailure = TiberFailure<
  "TIBER_DECLARATION_PATH_INVALID" | "TIBER_DECLARATION_PATH_IO",
  { readonly domain: "repository-declaration-path" },
  "corrected-input" | "state-change" | "retry-operation"
>;
export type RepositoryDeclarationPathResult = TiberResult<
  Option<CanonicalRepositoryDeclarationPath>,
  RepositoryDeclarationPathFailure
>;

function failure(
  code: RepositoryDeclarationPathFailure["code"],
  message: string,
): RepositoryDeclarationPathResult {
  return {
    ok: false,
    failure: operationalFailure(
      code,
      "repository-declaration-path",
      message,
      code === "TIBER_DECLARATION_PATH_IO" ? "transient" : "retry-after-input",
    ),
  };
}

function contained(root: string, candidate: string): boolean {
  const path = relative(root, candidate);
  return path === "" || (!path.startsWith("..") && !isAbsolute(path));
}

export function canonicalRepositoryDeclarationPath(
  repository: string,
  filename: "commands.json" | "workflow.json",
  operation: "read" | "remove" | "write",
): RepositoryDeclarationPathResult {
  try {
    const root = realpathSync(repository);
    const declaredParent = join(root, ".tiber");
    if (operation === "write")
      mkdirSync(declaredParent, { recursive: true, mode: 0o700 });
    if (!existsSync(declaredParent)) return { ok: true, value: none };
    const parent = realpathSync(declaredParent);
    if (!contained(root, parent))
      return failure(
        "TIBER_DECLARATION_PATH_INVALID",
        "repository declaration path escapes the repository",
      );
    const declaredTarget = join(parent, filename);
    const target =
      operation === "read" && existsSync(declaredTarget)
        ? realpathSync(declaredTarget)
        : declaredTarget;
    return contained(root, target)
      ? {
          ok: true,
          value: some(target as CanonicalRepositoryDeclarationPath),
        }
      : failure(
          "TIBER_DECLARATION_PATH_INVALID",
          "repository declaration target escapes the repository",
        );
  } catch {
    return failure(
      "TIBER_DECLARATION_PATH_IO",
      "repository declaration path could not be resolved",
    );
  }
}
