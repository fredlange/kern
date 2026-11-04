handler rand.below n:Int -> Int
  need n > 0 else BadBound
  ensure result >= 0 and result < n
