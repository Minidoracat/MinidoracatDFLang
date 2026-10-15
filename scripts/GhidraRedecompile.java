// Ghidra headless postScript：用較長的逾時與指令上限重新反編譯指定函式（全量匯出時逾時或流程過長而失敗的那幾個）。
// 由 scripts/df_snapshot.py `redecompile` 呼叫：... -postScript GhidraRedecompile.java <out.c> <逾時秒數> <指令上限> <位址>...
// 輸出格式同 GhidraExportDecomp.java，每個函式前一行是 "// ============ <name> @ 0x<va> ============"。
// @category DF

import ghidra.app.decompiler.DecompInterface;
import ghidra.app.decompiler.DecompileOptions;
import ghidra.app.decompiler.DecompileResults;
import ghidra.app.script.GhidraScript;
import ghidra.program.model.listing.Function;

import java.io.BufferedWriter;
import java.io.FileOutputStream;
import java.io.OutputStreamWriter;
import java.io.Writer;
import java.nio.charset.StandardCharsets;

public class GhidraRedecompile extends GhidraScript {
    @Override
    protected void run() throws Exception {
        String[] args = getScriptArgs();
        if (args.length < 4) {
            throw new IllegalArgumentException("usage: GhidraRedecompile.java <out.c> <timeout-seconds> <max-instructions> <address>...");
        }
        int timeout = Integer.parseInt(args[1]);
        DecompileOptions options = new DecompileOptions();
        options.setMaxInstructions(Integer.parseInt(args[2]));
        options.setMaxPayloadMBytes(1024);
        DecompInterface decompiler = new DecompInterface();
        decompiler.setOptions(options);
        decompiler.toggleSyntaxTree(false);
        decompiler.openProgram(currentProgram);
        try (Writer out = new BufferedWriter(
                new OutputStreamWriter(new FileOutputStream(args[0]), StandardCharsets.UTF_8))) {
            for (int i = 3; i < args.length; i++) {
                Function f = getFunctionAt(toAddr(args[i]));
                if (f == null) {
                    println("no function at " + args[i]);
                    continue;
                }
                DecompileResults r = decompiler.decompileFunction(f, timeout, monitor);
                String head = "// ============ " + f.getName(true) + " @ 0x"
                    + Long.toHexString(f.getEntryPoint().getOffset()) + " ============\n";
                String body = r.decompileCompleted()
                    ? r.getDecompiledFunction().getC() + "\n"
                    : "// DECOMPILE FAILED: " + String.valueOf(r.getErrorMessage()).strip() + "\n\n";
                out.write(head + body);
                out.flush();
                println((r.decompileCompleted() ? "ok " : "failed ") + f.getName(true));
            }
        } finally {
            decompiler.dispose();
        }
    }
}
