import { isValidElement, type ReactElement, type ReactNode } from "react";
import { describe, expect, it, vi } from "vitest";
import { BodyEditor } from "./BodyEditor";

type ElementProps = { children?: ReactNode; className?: string; "aria-label"?: string; onChange?: (event: { target: { value: string } }) => void };
function elements(node: ReactNode): ReactElement<ElementProps>[] {
  if (Array.isArray(node)) return node.flatMap(elements);
  if (!isValidElement<ElementProps>(node)) return [];
  return [node, ...elements(node.props.children)];
}

describe("multipart editing", () => {
  it("retains the row identity through successive name keystrokes", () => {
    let parts = [{ name: "", enabled: true, value: { kind: "text" as const, content: "value" } }];
    const props: Parameters<typeof BodyEditor>[0] = {
      mode: "multipart", rawContent: "", rawContentType: "", formFields: [], multipartParts: parts,
      binaryFilePath: "", binaryContentType: "", onModeChange: vi.fn(), onRawContentChange: vi.fn(),
      onRawContentTypeChange: vi.fn(), onFormFieldsChange: vi.fn(), onMultipartPartsChange: (next) => { parts = next as typeof parts; },
      onBinaryFilePathChange: vi.fn(), onBinaryContentTypeChange: vi.fn(), onPickFile: async () => null,
    };
    const render = () => elements(BodyEditor({ ...props, multipartParts: parts }));
    const key = render().find((element) => element.props.className === "multipart-part")!.key;
    for (const name of ["f", "fi", "file"]) {
      render().find((element) => element.props["aria-label"] === "Multipart field name")!.props.onChange!({ target: { value: name } });
      expect(render().find((element) => element.props.className === "multipart-part")!.key).toBe(key);
      expect(parts[0]?.name).toBe(name);
    }
  });
});
