export type Option<Value> =
  { readonly kind: "some"; readonly value: Value } | { readonly kind: "none" };

export const none: Option<never> = { kind: "none" };

export function some<Value>(value: Value): Option<Value> {
  return { kind: "some", value };
}

export function isSome<Value>(option: Option<Value>): option is {
  readonly kind: "some";
  readonly value: Value;
} {
  return option.kind === "some";
}

export function mapOption<Value, Mapped>(
  option: Option<Value>,
  map: (value: Value) => Mapped,
): Option<Mapped> {
  return option.kind === "some" ? some(map(option.value)) : none;
}
