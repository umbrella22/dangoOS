#!/usr/bin/env python3
"""Capture display-metrics console records and summarize physical firmware runs.

Uses the normal console, independently of EmbedScope's binary CDC. A synthetic
swipe measures queue-to-pixel-submit latency, not physical finger-to-photon time.
The pixel timestamp is a software DMA submission proxy, not a measured wire edge.
"""
import argparse
import json
import math
import re
import sys
import time
from pathlib import Path


def capture(args):
    from console import Device

    args.output.parent.mkdir(parents=True, exist_ok=True)
    with args.output.open("w") as log:
        class Probe(Device):
            def lines(self, deadline):
                for line in super().lines(deadline):
                    log.write(line + "\n")
                    log.flush()
                    yield line

        device = Probe(args.port, False)
        def command(text):
            log.write(f"HOST command={text}\n")
            response = device.command(text)
            print(response, flush=True)
        def observe(seconds):
            for _ in device.lines(time.monotonic() + seconds):
                pass
        try:
            command("boot-status")
            command("display-test status")
            if args.scenario == "home":
                command("display-test page home")
                observe(args.seconds)
            elif args.scenario == "push":
                command("display-test page launcher")
                observe(1)
                for _ in range(args.rounds):
                    for page in ("settings", "launcher"):
                        command("display-test page " + page)
                        observe(.8)
            elif args.scenario == "slider":
                # The caller first opens the slider and supplies its real UI bounds.
                x0, y0, x1, y1 = args.bounds
                for _ in range(args.rounds):
                    for start, end in (((x0,y0),(x1,y1)), ((x1,y1),(x0,y0))):
                        command(f"display-test swipe {start[0]} {start[1]} {end[0]} {end[1]} 500")
                        observe(.15)
            else:
                for text in args.command:
                    command(text)
                    observe(.8)
                observe(args.seconds)
            command("audio-status")
            command("wifi-status")
            command("bt-status")
            command("boot-status")
        finally:
            device.ser.close()
    analyze(args.output, args.output.with_suffix(".json"))


def distribution(values):
    if not values:
        return {"n": 0}
    values = sorted(values)
    def percentile(p):
        index = (len(values)-1)*p
        low, high = math.floor(index), math.ceil(index)
        return round(values[low] + (values[high]-values[low])*(index-low), 3)
    return {"n":len(values), "min":values[0], "p50":percentile(.5), "p95":percentile(.95), "max":values[-1]}


def analyze(path, output):
    frames, costs, inputs, busy, renders = {}, {}, {}, [], []
    incomplete = []
    for line in path.read_text().splitlines():
        match = re.search(r"display\.(frame|cost|input|busy|render) (.*)",line)
        if not match:
            continue
        kind, body = match.groups()
        fields = dict(re.findall(r"(\w+)=([\w.-]+)",body))
        fields = {k: int(v) if v.isdigit() else v for k,v in fields.items()}
        required = {"frame":("id","start","end","kind","te"),"cost":("id","bytes","pack","transfer","pixel","offset"),"input":("id","first","last","read","count"),"busy":("begin","end"),"render":("kind","end","us")}[kind]
        if not all(key in fields for key in required):
            incomplete.append(line)
            continue
        if kind == "busy": busy.append(fields)
        elif kind == "render": renders.append(fields)
        else: {"frame":frames,"cost":costs,"input":inputs}[kind][fields["id"]] = fields
    result = {"log":str(path), "pixel_timestamp":"software DMA submission proxy", "incomplete_lines":incomplete, "unpaired_frames":sorted(set(frames)^set(costs)), "scenarios":{}}
    for kind in ("home","screen","slide"):
        data = [(frame,costs[ident]) for ident,frame in frames.items() if frame["kind"]==kind and ident in costs]
        if not data: continue
        series = {key:[] for key in ("total_us","te_us","pack_us","transfer_us","bytes","direct_bytes","windows","pack_transfer_ratio","completion_interval_us","input_oldest_us","input_latest_us","input_to_read_us","busy_wait_us","busy_latency_share","render_us")}
        previous = None
        for frame,cost in data:
            for key,value in (("total_us",frame["end"]-frame["start"]),("te_us",frame["te"]),("pack_us",cost["pack"]),("transfer_us",cost["transfer"]),("bytes",cost["bytes"]),("direct_bytes",cost.get("direct",0)),("windows",cost.get("windows",0))): series[key].append(value)
            if cost["transfer"]: series["pack_transfer_ratio"].append(cost["pack"]/cost["transfer"])
            if previous is not None and 0 < frame["end"]-previous < 200_000:
                series["completion_interval_us"].append(frame["end"]-previous)
            previous = frame["end"]
            trace = inputs.get(frame["id"])
            if trace and trace["first"] <= trace["read"] <= cost["pixel"]:
                oldest,latest = cost["pixel"]-trace["first"],cost["pixel"]-trace["last"]
                overlap = sum(max(0,min(interval["end"],trace["read"])-max(interval["begin"],trace["first"])) for interval in busy)
                series["input_oldest_us"].append(oldest)
                series["input_latest_us"].append(latest)
                series["input_to_read_us"].append(trace["read"]-trace["first"])
                series["busy_wait_us"].append(overlap)
                if oldest: series["busy_latency_share"].append(overlap/oldest)
        series["render_us"] = [r["us"] for r in renders if r["kind"]==kind]
        result["scenarios"][kind] = {key:distribution(values) for key,values in series.items()}
    output.write_text(json.dumps(result,indent=2)+"\n")
    print(json.dumps({"output":str(output),"frames":len(frames),"unpaired":len(result["unpaired_frames"]),"incomplete":len(incomplete)},indent=2))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="action",required=True)
    run = sub.add_parser("capture")
    run.add_argument("--port",required=True)
    run.add_argument("--output",type=Path,required=True)
    run.add_argument("--scenario",choices=("home","push","slider","commands"),default="commands")
    run.add_argument("--rounds",type=int,default=10)
    run.add_argument("--seconds",type=float,default=10)
    run.add_argument("--command",action="append",default=[])
    run.add_argument("--bounds",type=int,nargs=4)
    report = sub.add_parser("analyze")
    report.add_argument("log",type=Path)
    report.add_argument("--output",type=Path,required=True)
    args = parser.parse_args()
    if args.action=="capture":
        if args.scenario=="slider" and args.bounds is None: parser.error("slider needs --bounds x0 y0 x1 y1 from the actual UI tree")
        capture(args)
    else: analyze(args.log,args.output)


if __name__=="__main__": main()
