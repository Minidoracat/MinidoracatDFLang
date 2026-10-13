// Ghidra headless preScript：匯入後、自動分析前套用 exe/labels.tsv，並關掉分析期的反編譯。
// 由 scripts/df_snapshot.py `decompile` 呼叫：
//   analyzeHeadless <proj> df -import "Dwarf Fortress.exe" -preScript GhidraPrepare.java <labels.tsv> ...
// labels.tsv 欄位：va<TAB>name<TAB>source；fn_ 開頭建函式，其餘（g_ 全域、vt_ vtable）建標籤。
// @category DF

import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.Function;
import ghidra.program.model.symbol.SourceType;

import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;

public class GhidraPrepare extends GhidraScript {
    @Override
    protected void run() throws Exception {
        String[] args = getScriptArgs();
        if (args.length != 1) {
            throw new IllegalArgumentException("usage: GhidraPrepare.java <labels.tsv>");
        }
        // 全函式反編譯在 postScript 平行做；分析期的 Decompiler Parameter ID 會再逐一反編譯一次，時間翻倍
        setAnalysisOption(currentProgram, "Decompiler Parameter ID", "false");

        int functions = 0;
        int labels = 0;
        for (String line : Files.readAllLines(Path.of(args[0]), StandardCharsets.UTF_8)) {
            if (line.isBlank() || line.startsWith("va\t")) {
                continue;
            }
            String[] f = line.split("\t");
            Address addr = toAddr(Long.parseUnsignedLong(f[0].substring(2), 16));
            String name = f[1];
            if (name.startsWith("fn_")) {
                disassemble(addr);
                Function fn = getFunctionAt(addr);
                if (fn == null) {
                    fn = createFunction(addr, name);
                } else {
                    fn.setName(name, SourceType.IMPORTED);
                }
                if (fn != null) {
                    fn.setComment("source: " + (f.length > 2 ? f[2] : "?"));
                    functions++;
                }
            } else {
                createLabel(addr, name, true, SourceType.IMPORTED);
                labels++;
            }
        }
        println("applied " + functions + " functions, " + labels + " labels");
    }
}
