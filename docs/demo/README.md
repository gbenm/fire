# README demo

Source of `docs/assets/demo.gif`, a [HyperFrames](https://hyperframes.heygen.com) composition
(HTML + GSAP). The terminal text mirrors real `fire` output; update it when the CLI output changes.

```bash
cd docs/demo
npx hyperframes@0.9.21 check
npx hyperframes@0.9.21 render -o demo.mp4
ffmpeg -y -i demo.mp4 -vf "fps=12,scale=1100:-1:flags=lanczos,split[a][b];[a]palettegen=max_colors=128:stats_mode=diff[p];[b][p]paletteuse=dither=bayer:bayer_scale=5:diff_mode=rectangle" ../assets/demo.gif
```
