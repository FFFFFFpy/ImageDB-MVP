# 图像匹配

## 指纹

每张图片保存：

- file_size
- blake3
- pixel_hash
- gradient_hash
- block_hash
- median_hash
- width
- height
- format
- fingerprint_version

## 候选范围

- 同一待导入图集内部
- 待导入图片与历史图库

## 比较顺序

1. 文件大小与 BLAKE3。
2. 标准像素 Hash。
3. 感知 Hash 汉明距离。
4. 旋转和镜像变换匹配。
5. 自动决策或人工审核。

## 自动决策

自动排除必须记录：

- 匹配类型
- 使用的指纹版本
- 各项距离
- 变换关系
- 保留图片
- 决策规则版本

未达到明确规则的候选进入审核队列。
