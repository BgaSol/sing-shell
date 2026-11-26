export async function readNodeFile(
  file: Pick<File, "size" | "arrayBuffer">,
): Promise<string> {
  if (file.size > 2 * 1024 * 1024)
    throw new Error("文件超过 2 MiB，请拆分后导入");
  try {
    return new TextDecoder("utf-8", { fatal: true }).decode(
      await file.arrayBuffer(),
    );
  } catch {
    throw new Error("文件读取失败或不是有效 UTF-8，请重新选择");
  }
}
