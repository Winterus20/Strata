Add-Type -AssemblyName System.Drawing

$textures = @{
    "stone.png" = @{
        baseColor = [System.Drawing.Color]::FromArgb(128, 128, 128)
        noisePixels = @(  # darker speckles
            @{x=2;y=3;r=100;g=100;b=100}, @{x=7;y=5;r=90;g=90;b=90},
            @{x=12;y=8;r=100;g=95;b=95}, @{x=5;y=12;r=110;g=110;b=110},
            @{x=10;y=2;r=95;g=95;b=95}, @{x=14;y=14;r=100;g=100;b=100}
        )
    }
    "dirt.png" = @{
        baseColor = [System.Drawing.Color]::FromArgb(139, 90, 43)
        noisePixels = @(
            @{x=3;y=5;r=120;g=80;b=35}, @{x=8;y=3;r=150;g=100;b=50},
            @{x=11;y=9;r=130;g=85;b=40}, @{x=4;y=13;r=145;g=95;b=45}
        )
    }
    "grass_top.png" = @{
        baseColor = [System.Drawing.Color]::FromArgb(90, 180, 60)
        noisePixels = @(
            @{x=2;y=4;r=70;g=160;b=50}, @{x=9;y=2;r=100;g=200;b=70},
            @{x=13;y=7;r=80;g=170;b=55}, @{x=6;y=11;r=95;g=190;b=65}
        )
    }
    "grass_side.png" = @{
        baseColor = [System.Drawing.Color]::FromArgb(120, 150, 70)
        noisePixels = @(
            @{x=1;y=0;r=60;g=140;b=40}, @{x=3;y=0;r=70;g=150;b=50},
            @{x=7;y=1;r=80;g=160;b=55}, @{x=10;y=0;r=65;g=145;b=45},
            @{x=14;y=1;r=75;g=155;b=50}
        )
    }
    "grass_bottom.png" = @{
        baseColor = [System.Drawing.Color]::FromArgb(139, 90, 43)
        noisePixels = @(
            @{x=5;y=3;r=120;g=80;b=35}, @{x=10;y=8;r=130;g=85;b=40},
            @{x=3;y=12;r=145;g=95;b=45}
        )
    }
    "bedrock.png" = @{
        baseColor = [System.Drawing.Color]::FromArgb(60, 60, 60)
        noisePixels = @(
            @{x=1;y=1;r=40;g=40;b=40}, @{x=14;y=2;r=50;g=50;b=50},
            @{x=7;y=7;r=45;g=45;b=45}, @{x=3;y=14;r=55;g=55;b=55},
            @{x=12;y=13;r=40;g=40;b=40}, @{x=9;y=4;r=50;g=50;b=50},
            @{x=5;y=9;r=45;g=45;b=45}, @{x=13;y=10;r=55;g=55;b=55}
        )
    }
    "wood.png" = @{
        baseColor = [System.Drawing.Color]::FromArgb(180, 140, 80)
        noisePixels = @(
            @{x=0;y=0;r=160;g=120;b=60}, @{x=0;y=4;r=170;g=130;b=70},
            @{x=0;y=8;r=165;g=125;b=65}, @{x=0;y=12;r=175;g=135;b=75},
            @{x=15;y=2;r=155;g=115;b=55}, @{x=15;y=6;r=165;g=125;b=65},
            @{x=15;y=10;r=170;g=130;b=70}, @{x=15;y=14;r=160;g=120;b=60}
        )
    }
    "leaves.png" = @{
        baseColor = [System.Drawing.Color]::FromArgb(50, 130, 50)
        noisePixels = @(
            @{x=1;y=2;r=40;g=110;b=40}, @{x=5;y=6;r=60;g=140;b=60},
            @{x=10;y=3;r=45;g=120;b=45}, @{x=12;y=11;r=55;g=135;b=55},
            @{x=7;y=13;r=40;g=115;b=40}, @{x=3;y=9;r=50;g=125;b=50},
            @{x=14;y=5;r=45;g=110;b=45}, @{x=8;y=1;r=55;g=130;b=55}
        )
    }
    "sand.png" = @{
        baseColor = [System.Drawing.Color]::FromArgb(220, 210, 120)
        noisePixels = @(
            @{x=4;y=3;r=200;g=190;b=100}, @{x=9;y=7;r=230;g=220;b=130},
            @{x=13;y=12;r=210;g=200;b=110}, @{x=2;y=10;r=225;g=215;b=125},
            @{x=11;y=4;r=205;g=195;b=105}
        )
    }
    "water.png" = @{
        baseColor = [System.Drawing.Color]::FromArgb(40, 80, 180)
        noisePixels = @(
            @{x=3;y=5;r=30;g=70;b=170}, @{x=8;y=2;r=50;g=90;b=190},
            @{x=11;y=9;r=35;g=75;b=175}, @{x=5;y=13;r=45;g=85;b=185},
            @{x=13;y=6;r=30;g=70;b=170}, @{x=2;y=11;r=40;g=80;b=180}
        )
    }
}

foreach ($name in $textures.Keys) {
    $tex = $textures[$name]
    $bmp = New-Object System.Drawing.Bitmap(16, 16)
    for ($x = 0; $x -lt 16; $x++) {
        for ($y = 0; $y -lt 16; $y++) {
            $bmp.SetPixel($x, $y, $tex.baseColor)
        }
    }
    foreach ($p in $tex.noisePixels) {
        $c = [System.Drawing.Color]::FromArgb($p.r, $p.g, $p.b)
        $bmp.SetPixel($p.x, $p.y, $c)
    }
    $path = Join-Path "assets\textures" $name
    $bmp.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
    $bmp.Dispose()
    Write-Host "Created $path"
}

Write-Host "All textures generated!"
