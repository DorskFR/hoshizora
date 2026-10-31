import type { GraphNode } from "../types";

const TEST_PATH =
  /(^|\/)(tests?|__tests__|e2e|journeys|spec|testdata|fixtures)\/|\.(test|spec|journey|e2e)\.[cm]?[jt]sx?$|_test\.(go|py|rs)$|(^|\/)test_[^/]*\.py$|(^|\/)conftest\.py$/;
const TEST_NAME = /(^|::|\.)(tests?|test_[\w]+)(::|\.|$)/;

/** Tests, test helpers and fixtures: hidden by default everywhere a human browses the code. */
export function isTestNode(n: Pick<GraphNode, "entrypoint" | "file" | "qualified_name">): boolean {
  if (n.entrypoint === "test") return true;
  if (n.file && TEST_PATH.test(n.file)) return true;
  return TEST_NAME.test(n.qualified_name);
}
